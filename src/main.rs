use std::io::{stdout, Write};
use std::path::Path;
use std::fs;
use std::thread::sleep;
use std::time::Duration;
use octocrab::{self, models::pulls::PullRequest, params, Octocrab};
use serde_yaml::{self};
use chrono::{DateTime, Days, Local, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use clokwerk::Interval::{self};
use clokwerk::{Job, Scheduler, TimeUnits};
use lazy_static;

#[allow(dead_code)]
const COW: &str = "((...))\n( o o )\n \\   / \n  ^_^  ";

const FILE_NAME: &str = "SimpleGitHubBot.yml";

// Debug vars
const REGENERATE_CONFIG: bool = !true;
const USE_TEMPLATE: bool = !false;

// The template used to generate the YAML file when the application is first run.
//-- Copy this line out of this file, Find and Replace (with RegEx) '\\n' with '\n' to break it up, then do the opposite to put it back together.
const YAML_TEMPLATE: &str = "### NOTE THAT THIS FILE WILL BE ALTERED7\n\n### The bot uses this file to store per-run data, and regenerates it every run.\n\
                            ### The information you enter will be used and remembered, but do not rely on it being static.\n\n\
                            ## Your GitHub API token\napi_token: token-here\n\
                            ## The repo we'll be cloning PRs from\nclone_repo:\n  ## The owner or org of the repository\n  owner: space-wizards\n  ## The name of the repository\n  name: space-station-14\n  ## The branch to check for PRs on\n  branch: master\n\
                            ## The repo we'll be making our PR to\ninto_repo:\n  ## The owner or org of the repository to clone PRs into\n  owner: Simple-Station\n  ## The name of the repository to clone PRs into\n  name: Parkstation\n  ## The branch to clone PRs into\n  branch: master\n\
                            ## The date to start checking for PRs from\n## Note that if this is too low, you'll get *every PR ever made*. This will be a lot of PRs. Format is YYYY-MM-DD\ndate_from: 2024-01-01\n\
                            ## The number of days between checks for new PRs\n## '7' would run once a week\n## A value of '0' will run once before exiting\ndays_betwen: 7";

fn main() {
    let config = generate_config();

    if config.days_between == 0 {
        println!("'days_between' is set to 0, running once then exiting.");
        run_tasks();
        return;
    }

    if config.date_from_with_time().and_utc() >= Utc::now() { //FIXME: This isn't comparing correctly I guess??
        println!("'date_from' is set to a date in the future ({}), the mirror will first run {} days after that point.", config.date_from_with_time(), config.days_between);
        // Create a task that runs once at the configured date_from plus the days_between, then repeates every days_between thereafter.
        let cur_time = Utc::now();
        let first_run = config.date_from_with_time().and_utc().checked_add_days(config.days_between_days()).expect("Are your dates and times valid?");
        let until_first_run = (first_run - cur_time).num_days() as u32; // Fucking *needs* to be u32.
        let until_first_run_int = until_first_run.days();

        println!("First run will be at {}, in {} days.", first_run.naive_local(), until_first_run);

        let mut prime_scheduler = Scheduler::with_tz(Utc);
        prime_scheduler.every(until_first_run_int).once().run(move || loop_schedules(setup_tasks(config.clone())));
    }
    else {
        println!("Running mirror now and setting up repeating task to run every {} days.", config.days_between);

        run_tasks();
        loop_schedules(setup_tasks(config));
    }
}

fn loop_schedules(mut scheduler: Scheduler::<Utc>) {
    loop {
        scheduler.run_pending();
        print!("Chked!");
        let _ = stdout().flush();
        sleep(Duration::from_millis(500));
    }
}

fn setup_tasks(config: AppConfig) -> Scheduler::<Utc> {
    let mut scheduler = Scheduler::with_tz(Utc);

    scheduler.every(config.days_between_interval()).run(run_tasks);

    return scheduler;
}

fn run_tasks() {
    let config = generate_config();
    println!("Running scheduled tasks at {}.", Local::now().to_rfc2822());
    
    mirror_prs(config.clone());
}

#[tokio::main]
async fn mirror_prs(config: AppConfig) {
    println!("Mirroing all merged PRs since {} from {}/{}/{} to {}/{}/{}.",
        config.date_from_with_time(),
        config.clone_repo.owner, config.clone_repo.name, config.clone_repo.branch,
        config.into_repo.owner, config.into_repo.name, config.into_repo.branch);

    let mut octocrab = octocrab::OctocrabBuilder::new();
    octocrab = octocrab.user_access_token(config.api_token.clone());
    let octocrab = octocrab.build();
    eprintln!("\nDid build\n");
    let octocrab = octocrab.expect("Octocrab failed to build");

    let mut all_prs = get_all_prs(&octocrab, config.clone()).await;

    // if 
    println!("Found {} PRs starting at {} and ending at {}.", all_prs.len(), all_prs.first().unwrap().number, all_prs.last().unwrap().number);

    let date_time_cutoff: DateTime<Utc> = config.date_from_with_time().and_utc();

    let debug = config.debug.unwrap_or(false);

    all_prs.retain(|pr| pr.merged_at.is_some());
    all_prs.retain(|pr| { if debug { dbg!(pr.number); dbg!(pr.merged_at.unwrap()); dbg!(date_time_cutoff); } pr.merged_at.unwrap() >= date_time_cutoff });
    all_prs.sort_unstable_by_key(|pr| pr.merged_at);

    println!("Filtered down to {} PRs starting at {} and ending at {}.", all_prs.len(), all_prs.first().unwrap().number, all_prs.last().unwrap().number);

    let mut text = String::new();
    for pr in all_prs.iter() {
        text += "--------------------------------------\n";
        text += &format!("Merged pr #{}: {:?}\n", pr.number, pr.title.clone().unwrap());
        text += &format!("Merged at: {} by a {:?}\n", pr.merged_at.unwrap(), pr.author_association.clone().unwrap());
        text += &format!("Adds {:#?} lines and removes {:#?} lines\n", pr.additions, pr.deletions);
        text += &format!("It had {:#?} comments, {:#?} commits and {:#?} changed files\n", pr.comments, pr.commits, pr.changed_files);
        text += &format!("It was opened at {:#?}\n", pr.created_at.unwrap());
    }
    println!("{}", text);

    finalize(config);
}

async fn get_all_prs(octocrab: &Octocrab, config: AppConfig) -> Vec<PullRequest> {
    // Returns the first page of all prs.
    let mut page = octocrab.pulls(&config.clone_repo.owner, &config.clone_repo.name).list()
        .base(&config.clone_repo.branch)
        .state(params::State::Closed)
        .per_page(100)
        .send().await.expect("Failed to find repository PRs.");

    // Start our collection.
    let mut all_prs = page.take_items();

    // As long as we're not in debug mode, get all pages.
    // This takes a very long time, so we skip it for testing.
    if !config.debug.unwrap_or(false) {
        let msg = &"Failed to get all pages- are you being rate limited?";
        let all_pages = octocrab.all_pages(page).await.expect(msg);
        all_prs.extend(all_pages.iter().cloned());
    }

    return all_prs;
}

fn finalize(mut config: AppConfig) {
    // Set our date_from to the current date, so we only pick up new PRs next time we run.
    config.date_from = Utc::now().date_naive();
    // Set our exact time offset, to ensure we don't miss any PRs.
    config.time_offset = Utc::now().time().into();

    println!("Updating {}.", FILE_NAME);

    // Write the new config to the file.
    let yaml_contents = serde_yaml::to_string(&config).unwrap();
    write_to_config(yaml_contents);
}

fn generate_config() -> AppConfig {
    // Create the file if it doesn't exist.
    if !Path::new(FILE_NAME).exists() || REGENERATE_CONFIG {
        println!("{}", format!("Config file {} does not exist, attempting to create it at {}.", FILE_NAME, std::env::current_dir().unwrap().to_str().unwrap()));
        if USE_TEMPLATE {
            write_to_config(YAML_TEMPLATE.to_string());
        }
        else {
            write_to_config(serde_yaml::to_string(&AppConfig::default()).unwrap());
        }
        panic!("Config file created. Please fill in the necessary information and run the program again.");
    }

    let yaml_contents = fs::read_to_string(FILE_NAME).unwrap();
    return serde_yaml::from_str(&yaml_contents).unwrap();
}

fn write_to_config(contents: String) {
    fs::write(FILE_NAME, contents).expect(&format!("Config file {} could not be created, or could not be written to.\nIf the program lacks sufficient permissions to create the file, it will likely be unable to write to it later.", FILE_NAME));
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Default, Clone)]
pub struct AppConfig {
    api_token: String,
    clone_repo: RepoInfo,
    into_repo: RepoInfo,
    date_from: NaiveDate,
    days_between: u32,
    time_offset: Option<NaiveTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    debug: Option<bool>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Default, Clone)]
pub struct RepoInfo {
    owner: String,
    name: String,
    branch: String,
}

impl AppConfig {
    fn date_from_with_time(&self) -> NaiveDateTime {
        return self.date_from.and_time(self.time_offset.unwrap_or(NaiveTime::default()));
    }

    fn days_between_interval(&self) -> Interval {
        return self.days_between.days();
    }

    fn days_between_days(&self) -> Days {
        return Days::new(self.days_between as u64);
    }
}

