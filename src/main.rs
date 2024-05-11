use chrono::{DateTime, Days, Local, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use clokwerk::{Interval, Job, Scheduler, TimeUnits};
use futures::executor::block_on;
use git2::{Error as GitError, Repository};
use octocrab::{self, models::pulls::PullRequest, models::Author, params, params::pulls::Sort, params::Direction, Octocrab};
use serde_yaml;
use std::{fs, io::Write, path::Path, thread::sleep, time::Duration};
use tokio::time::timeout;

mod git_utils;
mod pr_template;

#[allow(dead_code)]
const COW: &str = "((...))\n( o o )\n \\   / \n  ^_^  ";

const FILE_NAME: &str = "simple_mirror_config.yml";

// Debug vars
/// Prevents any lasting net activity, such as pushing to branches, and opening PRs.
/// Prints potential PRs and issues to the console instead.
pub const NO_NET_ACTIVITY: bool = !true;
/// Prints finished PRs and issues to the console. Most useful when NO_NET_ACTIVITY is not false.
const PRINT_PRS: bool = !true;
/// Only gathers and iterates over the first 100 PRs. Generally much faster.
const FIRST_100_ONLY: bool = !true;
/// If this list is populated with pr numbers, the program will only cherry-pick and push those PRs instead of fetching any.
const CHERRY_PICK_ONLY: [u64; 0] = [];

// The template used to generate the YAML file when the application is first run.
const YAML_TEMPLATE: &str = "\
                                ### NOTE THAT THIS FILE WILL BE ALTERED\n\n### The bot uses this file to store per-run data, and regenerates it every run.\n\
                                ### The information you enter will be used and remembered, but do not rely on it being static.\n\n\
                                # Yes, this bot requires two access tokens, one owned by the account and one owned by the organization.\n\
                                # Yes, this blows. Talk to Github about it\n\
                                ## The GitHub access token owned by the organization.\norg_token: token-here\n\
                                ## The GitHub access token owned by the bot user account.\nbot_token: token-here\n\
                                ## The repo we'll be cloning PRs from\nclone_repo:\n  ## The owner or org of the repository\n  owner: space-wizards\n  ## The name of the repository\n  name: space-station-14\n  ## The branch to check for PRs on\n  branch: master\n\
                                ## The repo we'll be making our PR to\ninto_repo:\n  ## The owner or org of the repository to clone PRs into\n  owner: Simple-Station\n  ## The name of the repository to clone PRs into\n  name: Parkstation\n  ## The branch to clone PRs into\n  branch: master\n\
                                ## The date to start checking for PRs from\n## Note that if this is too low, you'll get *every PR ever made*. This will be a lot of PRs. Format is YYYY-MM-DD\ndate_from: 2006-06-17\n\
                                ## The number of days between checks for new PRs\n## '7' would run once a week\n## A value of '0' will run once before exiting\ndays_between: 7\n\
                                ## A list of labels to apply to PRs made by the bot\npr_labels: [ ]\n\
                                ## A list of labels to apply to Issues made by the bot\nissue_labels: [ ]\n\
                                ## A list of labels to ignore PRs with\n## If a PR has any of these labels, it won't be mirrored\nignored_labels: [ ]\n\
                                ## A list of users to ignore PRs from\nignored_users: [ 'github-actions[bot]' ]\n\
                                ## The page number to stop collecting PRs at. This is in groups of 100, sorted by when they were created. \n## If this is empty, we'll get every PR every ever made and check their merge date.\n\
                                ## Note that setting this to only get the first page or two *is not necessarily the best idea*, since an earlier-made PR could be merged *after* a later-made one, and thus missing them is possible.\n\
                                ## This is to avoid gathering thousands of PRs you know you will never want.\nhard_cap: 0\
                            ";

#[tokio::main]
async fn main() {
    let config = generate_config();

    if config.days_between == 0 {
        let config = generate_config();

        let octocrab = octocrab::OctocrabBuilder::new()
            .user_access_token(config.org_token.clone())
            .build()
            .expect("Octocrab failed to build");

        let bot_info = block_on(get_bot_info(&config));

        println!("'days_between' is set to 0, running once then exiting.");
        mirror_prs(&octocrab, &config, &bot_info).await; //? Completely circumvents the scheduling and file writing all together.
        return;
    }

    if config.date_from_with_time().and_utc() >= Utc::now() {
        //FIXME: This isn't comparing correctly I guess??
        println!("'date_from' is set to a date in the future ({}), the mirror will first run {} days after that point.", config.date_from_with_time(), config.days_between);
        // Create a task that runs once at the configured date_from plus the days_between, then repeates every days_between thereafter.
        let cur_time = Utc::now();
        let first_run = config
            .date_from_with_time()
            .and_utc()
            .checked_add_days(config.days_between_days())
            .expect("Are your dates and times valid?");
        let until_first_run = (first_run - cur_time).num_days() as u32; // Fucking *needs* to be u32.
        let until_first_run_int = until_first_run.days();

        println!("First run will be at {}, in {} days.", first_run.naive_local(), until_first_run);
        println!("This program will now loop indefinitely. It should obviously be run in the background.");

        let mut prime_scheduler = Scheduler::with_tz(Utc);
        prime_scheduler
            .every(until_first_run_int)
            .once()
            .run(move || loop_schedules(setup_tasks(config.clone())));
    } else {
        println!("Running mirror now and setting up repeating task to run every {} days.", config.days_between);
        println!("This program will now loop indefinitely. It should obviously be run in the background.");

        run_tasks();
        loop_schedules(setup_tasks(config));
    }
}

fn loop_schedules(mut scheduler: Scheduler<Utc>) {
    loop {
        scheduler.run_pending();
        // print!(".");
        // let _ = stdout().flush();
        sleep(Duration::from_millis(500));
    }
}

fn setup_tasks(config: AppConfig) -> Scheduler<Utc> {
    let mut scheduler = Scheduler::with_tz(Utc);

    scheduler
        .every(config.days_between_interval())
        .run(run_tasks);

    return scheduler;
}

fn run_tasks() {
    let config = generate_config();

    let octocrab = octocrab::OctocrabBuilder::new()
        .user_access_token(config.org_token.clone())
        .build()
        .expect("Octocrab failed to build");

    let bot_info = block_on(get_bot_info(&config));

    println!("Running scheduled tasks at {}.", Local::now().to_rfc2822());

    block_on(mirror_prs(&octocrab, &config, &bot_info));

    finalize(config);
}

async fn mirror_prs(octocrab: &Octocrab, config: &AppConfig, bot_info: &Author) {
    println!("Mirroring all merged PRs since {} from {}/{}/{} to {}/{}/{}.",
        config.date_from_with_time(),
        config.clone_repo.owner, config.clone_repo.name, config.clone_repo.branch,
        config.into_repo.owner, config.into_repo.name, config.into_repo.branch);

    let mut all_prs = get_all_prs(&octocrab, &config).await;

    if all_prs.is_empty() {
        println!("No PRs found at all!");
        return;
    }

    println!("Found {} PRs starting at {} and ending at {}.",
        &all_prs.len(),
        &all_prs.first().unwrap().number,
        &all_prs.last().unwrap().number);

    let date_time_cutoff: DateTime<Utc> = config.date_from_with_time().and_utc();

    let debug = config.debug.unwrap_or(false);

    // I know the following lines are gross.
    // Debug info :)
    if debug { println!("\nChecking for unmerged PRs."); }
    all_prs.retain(|pr| { if debug && !pr.merged_at.is_some() { print!("Ignoring unmerged PR #{}, ", pr.number); } return pr.merged_at.is_some(); });
    if debug { println!("\n\nChecking for cutoff date {}", date_time_cutoff); }
    all_prs.retain(|pr| { if debug && pr.merged_at.unwrap() < date_time_cutoff { print!("Ignoring PR #{} merged before cutoff at {}, ", pr.number, pr.merged_at.unwrap()) } return pr.merged_at.unwrap() >= date_time_cutoff; });
    if debug { println!("\n\nChecking for ignored users: {:?}", config.ignored_users); }
    all_prs.retain(|pr| pr.user.to_owned().is_some_and(|user| { if config.ignored_users.contains(&user.login) { if debug { print!("Ignoring PR #{} made by ignored user {}, ", pr.number, &user.login) } return false } return true })); // This will also ignore any prs that don't have users I guess??
    if debug { println!("\n\nChecking for ignored labels: {:?}", config.ignored_labels); }
    all_prs.retain(|pr| pr.labels.to_owned().is_some_and(|labels| { if labels.iter().any(|label| config.ignored_labels.contains(&label.name)) { if debug { print!("Ignoring PR #{} with ignored label, ", pr.number) } return false } return true }));
    if debug { println!("\n\n"); }
    all_prs.sort_unstable_by_key(|pr| pr.merged_at);

    if all_prs.is_empty() {
        println!("No valid PRs found.");
        return;
    }

    println!("Filtered down to {} PRs starting at {} and ending at {}.",
        all_prs.len(),
        all_prs.first().unwrap().number,
        all_prs.last().unwrap().number);

    let repo = match git_utils::ensure_repo(&config, &bot_info) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to get or create local repository: {}", e);
            return;
        }
    };

    for merged_pr in all_prs.iter() {
        println!("Cherry-picking and pushing PR #{}.", merged_pr.number);
        match cherry_pick_and_push_pr(&repo, &octocrab, merged_pr.clone(), &config, &bot_info) {
            Ok(_) => {
                println!("Cherry-picked and pushed PR #{}.", merged_pr.number);
            }
            Err(e) => {
                eprintln!("Failed to cherry-pick and push PR #{} {}: {}", merged_pr.number, merged_pr.title.clone().unwrap_or_default(), e);
                make_issue(&config, &octocrab, merged_pr.clone(), e).await; // Report if something goes wrong.
            }
        }

        if git_utils::reset_repo(&repo, &config).is_err() {
            eprintln!("Failed to reset repository after cherry-picking PR #{}.", merged_pr.number);
            return;
        }

        println!(""); // New line to seperate them.
    }
}

fn cherry_pick_and_push_pr(repo: &Repository, octocrab: &Octocrab, merged_pr: PullRequest, config: &AppConfig, bot_info: &Author) -> Result<(), GitError> {
    let sha = match merged_pr.merge_commit_sha.to_owned() {
        Some(s) => s,
        None => {
            eprintln!("PR #{} has no merge commit SHA.", merged_pr.number);
            return Err(GitError::from_str("No merge commit SHA."));
        }
    };

    let branch_name = format!("{}_{}_{}_{}",
        &config.clone_repo.owner,
        &config.clone_repo.name,
        merged_pr.number,
        Utc::now().date_naive());

    println!("Creating branch {}.", branch_name);
    git_utils::create_branch(&repo, &branch_name)?;

    println!("Cherry-picking commit {}.", &sha);
    git_utils::cherry_pick_commit(&repo, &config, &bot_info, &sha)?;

    println!("Pushing to remote branch {}.", branch_name);
    git_utils::push_to_remote(&repo, &config, &bot_info)?;

    println!("Making pull request for {}.", branch_name);
    block_on(make_pull_request(&config, &octocrab, &bot_info, merged_pr, Some(sha), &branch_name));

    return Ok(());
}

async fn make_pull_request(config: &AppConfig, octocrab: &Octocrab, bot_info: &Author, original_pr: PullRequest, merge_sha: Option<String>, branch: &str) {
    let merge_commit = match &merge_sha {
        Some(s) => {
            let commit = octocrab
                .commits(&config.clone_repo.owner, &config.clone_repo.name)
                .get(s)
                .await;

            match commit {
                Ok(c) => Some(c),
                Err(_) => None,
            }
        }
        None => None,
    };

    let title = format!("Mirror: {}", original_pr.title.clone().unwrap_or_default());
    let head = format!("{}:{}", &bot_info.login, branch);
    let body = pr_template::PrTemplate::new(&original_pr, merge_commit).to_markdown();
    let base = config.into_repo.branch.clone();

    if !NO_NET_ACTIVITY {
        let pr_attempt = octocrab
            .pulls(&config.into_repo.owner, &config.into_repo.name)
            .create(&title, &head, &base)
            .body(&body)
            .draft(true)
            .maintainer_can_modify(true)
            .send()
            .await
            .inspect_err(|e| {
                eprintln!("Failed to create pull request for {}: {}\nSha: {}", original_pr.number, e, merge_sha.unwrap_or_default());
                eprintln!("This is probably a permissions issue.");
            });

        if pr_attempt.is_ok() {
            let pr = pr_attempt.unwrap();

            let _ = octocrab
                .issues(&config.into_repo.owner, &config.into_repo.name)
                .add_labels(pr.number, &config.pr_labels)
                .await
                .inspect_err(|e| eprintln!("Failed to add labels to PR #{}: {}", pr.number, e));
        }
    }

    if PRINT_PRS {
        println!("-------------\n{}\n{}\n-------------", title, &body);
    }
}

async fn make_issue(config: &AppConfig, octocrab: &Octocrab, pr: PullRequest, error: GitError) {
    let merge_commit = match pr.merge_commit_sha {
        Some(ref s) => {
            let commit = octocrab
                .commits(&config.clone_repo.owner, &config.clone_repo.name)
                .get(s)
                .await;

            match commit {
                Ok(c) => Some(c),
                Err(_) => None,
            }
        }
        None => None,
    };

    let title = format!("Failed to cherry-pick PR #{}: {}", pr.number, pr.title.clone().unwrap_or_default());
    let body = format!("## Failed to cherry-pick PR: {}\nPR body below\n\n{}", error, pr_template::PrTemplate::new(&pr, merge_commit).to_markdown());

    if !NO_NET_ACTIVITY {
        let issue_handler = octocrab
            .issues(&config.into_repo.owner, &config.into_repo.name)
            .create(&title)
            .body(&body)
            // .assignees(assignees) //TODO: Automatic assignees?
            .labels(Some(config.issue_labels.to_owned()))
            .send()
            .await;

        if issue_handler.is_err() {
            eprintln!("Failed to create issue for missed PR #{}: {}", pr.number, issue_handler.err().unwrap());
            eprintln!("This is probably a permissions issue.");
        }
    }

    if PRINT_PRS {
        println!("-------------\n{}\n{}\n-------------", title, &body);
    }
}

async fn get_all_prs(octocrab: &Octocrab, config: &AppConfig) -> Vec<PullRequest> {
    let forced_prs: Option<Vec<u64>> = match !CHERRY_PICK_ONLY.is_empty() {
        true => Some(CHERRY_PICK_ONLY.into()),
        false => match !config.prs_to_pull.is_empty() {
            true => Some(config.prs_to_pull.clone()),
            false => None,
        },
    };

    if forced_prs.is_some() {
        let mut prs = Vec::new();
        for num in CHERRY_PICK_ONLY.iter() {
            let pr = match octocrab
                .pulls(&config.clone_repo.owner, &config.clone_repo.name)
                .get(*num)
                .await
            {
                Ok(p) => p,
                Err(err) => {
                    eprintln!("Failed to get PR by number {}: {}", num, err);
                    continue;
                }
            };

            prs.push(pr);
        }

        return prs;
    }

    // Returns the first page of all prs.
    let mut page = match octocrab
        .pulls(&config.clone_repo.owner, &config.clone_repo.name)
        .list()
        .sort(Sort::Created)
        .direction(Direction::Descending)
        .base(&config.clone_repo.branch)
        .state(params::State::Closed)
        .per_page(100)
        .send()
        .await
    {
        Ok(p) => p,
        Err(err) => {
            eprintln!("Failed to get first page of PRs for {}/{}: {}",
                config.clone_repo.owner, config.clone_repo.name, err);
            return Vec::new();
        }
    };

    // Start our collection.
    let mut all_prs = page.take_items();

    // Getting all PRs takes a very long time, so we check if we should skip it.
    if FIRST_100_ONLY {
        println!("Retrieving only the first 100 PRs.");
        return all_prs;
    }

    println!("Attempting to gather all PR data- this may take a while...");

    // Determine how many pages there are, and how many times to call async.
    let total_pages = page.number_of_pages().unwrap_or(1);
    let total_pages = if let Some(hard_cap) = &config.hard_cap { hard_cap.min(&total_pages) } else { &total_pages };
    let pages_per_thread = total_pages / &config.max_async.unwrap_or(2);

    // Create a vector of futures to gather all PRs.
    let mut futures = Vec::new();
    let mut i = 2;
    while &i <= total_pages {
        let start = i;
        let end = i + pages_per_thread;
        i = end + 1;

        futures.push(get_prs_from_page_to(octocrab, config, start, end));

        let _ = std::io::stdout().flush();
    }

    // Gather all PRs.
    all_prs.extend(futures::future::join_all(futures).await.into_iter().flatten());

    // Remove duplicates.
    all_prs.sort_unstable_by_key(|pr| pr.number);
    all_prs.dedup_by(|a, b| a.number == b.number);

    println!("Done gathering all PRs!");

    return all_prs;
}

async fn get_prs_from_page_to(octocrab: &Octocrab, config: &AppConfig, page_start: u32, page_end: u32) -> Vec<PullRequest> {
    let mut page = octocrab
        .pulls(&config.clone_repo.owner, &config.clone_repo.name)
        .list()
        .sort(Sort::Created)
        .direction(Direction::Descending)
        .base(&config.clone_repo.branch)
        .state(params::State::Closed)
        .per_page(100)
        .page(page_start)
        .send()
        .await
        .inspect_err(|e| eprintln!("Failed to get page #{} of PRs: {}", page_start, e))
        .unwrap();

    let mut collection = vec![];
    collection.extend(page.take_items());

    let mut i = page_start + 1;
    while page.next.is_some() && i <= page_end {
        page = match octocrab.get_page(&page.next.clone()).await {
            Ok(p) => p.unwrap_or_else(|| panic!("Failed to get next page of PRs: No data returned.\nUnsure of how to continue.")),
            Err(err) => {
                eprintln!("Failed to get next page of PRs: {}\nAre you being rate limited?", err);
                return collection;
            }
        };

        collection.extend(page.take_items());

        print!("Done with page #{}, PR #{}...\r", i, collection.last().unwrap().number);
        let _ = std::io::stdout().flush();

        i += 1;
        // i = page //TODO: This sucks :P
        //     .prev
        //     .unwrap()
        //     .to_string()
        //     .split("page=")
        //     .last()
        //     .unwrap()
        //     .parse()
        //     .unwrap();
    }

    return collection;
}

fn finalize(mut config: AppConfig) {
    // Set our date_from to the current date, so we only pick up new PRs next time we run.
    config.date_from = Utc::now().date_naive();
    // Set our exact time offset, to ensure we don't miss any PRs.
    config.time_offset = Utc::now().time().into();

    println!("Updating {}.", FILE_NAME);

    // Write the new config to the file.
    let yaml_contents = serde_yaml::to_string(&config).unwrap();
    write_to_config(yaml_contents, Some(&config));
}

fn generate_config() -> AppConfig {
    // Create the file if it doesn't exist.
    if !Path::new(FILE_NAME).exists() {
        println!("Config file does not exist, attempting to create it at {}/{}.",
            std::env::current_dir()
                .expect("Couldn't find current dir! Are we lacking permissions?")
                .to_str()
                .unwrap_or_default(),
            FILE_NAME);
        write_to_config(YAML_TEMPLATE.to_string(), None);
        panic!("Config file {} created. Please fill in the necessary information and run the program again.", FILE_NAME);
    }

    let yaml_contents = fs::read_to_string(FILE_NAME).expect(&format!("Config file {} was confirmed to exist, but could not be read.\nAre we missing permissions?.", FILE_NAME));
    return match serde_yaml::from_str(&yaml_contents) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to parse config file: {}", e);

            //-- This timesout in case we're running in a non-interactive environment.
            //--  (timesout in 120 seconds)
            //TODO: This just straight up doesn't work right now, but it doesn't break anything so I left it in.
            if block_on(timeout(Duration::from_secs(120), request_regenerate_config())).is_err() {
                println!("Timed out waiting for user input.");
            }

            panic!();
        }
    };
}

async fn get_bot_info(config: &AppConfig) -> Author {
    let bot = Octocrab::builder()
        .user_access_token(config.bot_token.clone())
        .build()
        .expect("Octocrab failed to build")
        .current()
        .user()
        .await;

    if bot.is_err() {
        eprintln!("Couldn't obtain bot info: {}", bot.err().unwrap());
        panic!("Failed to get bot info.");
    }

    return bot.unwrap();
}

async fn request_regenerate_config() {
    println!("Config file {} is invalid, would you like to regenerate it? (THIS WILL DELETE THE CURRENT FILE) (y/N)", FILE_NAME);
    let mut input = String::new();
    match std::io::stdin().read_line(&mut input) {
        Ok(_) => {
            if input.trim().to_lowercase() == "y" {
                input = String::new();
                println!("Would you like to regenerate the file using a hardcoded template (with documentation) (1), or a default src-exact template (2)?");
                match std::io::stdin().read_line(&mut input) {
                    Ok(_) => {
                        if input.trim().to_lowercase() == "2" {
                            write_to_config(serde_yaml::to_string(&AppConfig::default()).unwrap(), None);
                        } else {
                            write_to_config(YAML_TEMPLATE.to_string(), None);
                        }
                        panic!("Config file {} has been regenerated. Please fill in the necessary information and run the program again.", FILE_NAME);
                    }
                    Err(e) => {
                        panic!("Failed to read input: {}", e);
                    }
                }
            } else {
                panic!("Config file {} is invalid, and will not be regenerated.", FILE_NAME);
            }
        }
        Err(e) => {
            panic!("Failed to read input: {}", e);
        }
    }
}

fn write_to_config(contents: String, config: Option<&AppConfig>) {
    if let Some(c) = config {
        if c.no_write.unwrap_or(false) {
            println!("No-write flag is set, not overwriting config.");
            println!("Contents would have been:\n{}", contents);
            return;
        }
    }

    fs::write(FILE_NAME, contents).expect(&format!("Config file {} could not be created, or could not be written to.\nAre we missing permissions?.", FILE_NAME));
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct AppConfig {
    org_token: String,
    bot_token: String,
    clone_repo: RepoInfo,
    into_repo: RepoInfo,
    date_from: NaiveDate,
    days_between: u32,
    #[serde(default)]
    pr_labels: Vec<String>,
    #[serde(default)]
    issue_labels: Vec<String>,
    #[serde(default)]
    ignored_labels: Vec<String>,
    #[serde(default)]
    ignored_users: Vec<String>,
    #[serde(default)]
    prs_to_pull: Vec<u64>,
    time_offset: Option<NaiveTime>,
    #[serde(default)]
    hard_cap: Option<u32>,
    #[serde(default)]
    max_async: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    debug: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    no_write: Option<bool>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Default, Clone)]
pub struct RepoInfo {
    owner: String,
    name: String,
    branch: String,
}

impl AppConfig {
    fn date_from_with_time(&self) -> NaiveDateTime {
        return self
            .date_from
            .and_time(self.time_offset.unwrap_or(NaiveTime::default()));
    }

    fn days_between_interval(&self) -> Interval {
        return self.days_between.days();
    }

    fn days_between_days(&self) -> Days {
        return Days::new(self.days_between as u64);
    }

    fn get_repo_path(&self) -> String {
        let path = format!("{}_{}_into_{}_{}",
            self.clone_repo.owner,
            self.clone_repo.name,
            self.into_repo.owner,
            self.into_repo.name
        );

        return path;
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        return AppConfig {
            org_token: String::default(),
            bot_token: String::default(),
            clone_repo: RepoInfo {
                owner: "space-wizards".to_string(),
                name: "space-station-14".to_string(),
                branch: "master".to_string(),
            },
            into_repo: RepoInfo {
                owner: "Simple-Station".to_string(),
                name: "Parkstation".to_string(),
                branch: "master".to_string(),
            },
            date_from: NaiveDate::from_ymd_opt(2006, 6, 17).unwrap(),
            days_between: 7,
            pr_labels: Vec::new(),
            issue_labels: Vec::new(),
            ignored_labels: Vec::new(),
            ignored_users: Vec::new(),
            prs_to_pull: Vec::new(),
            time_offset: None,
            hard_cap: None,
            max_async: None,
            debug: None,
            no_write: None,
        };
    }
}
