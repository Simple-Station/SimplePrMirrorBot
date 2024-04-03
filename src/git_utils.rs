use std::{cell::RefCell, io::{self, stdout, Write}, path::{Path, PathBuf}};
// use git2::{self, build::{CheckoutBuilder, RepoBuilder}, Error, FetchOptions, FileFavor, Progress, RemoteCallbacks, Repository, Signature, Time};
use git2::{self, Progress, *, build::*}; // Progress needs to be explicitly imported here since it conflicts with one in 'build::'
use crate::AppConfig;

const INTO_REMOTE_NAME: &str = "origin";
const CLONE_REMOTE_NAME: &str = "cloned";
const PUSH_REMOTE_NAME: &str = "owned";

/// Pushes the current branch to the owned remote with the same branch name.
pub fn push_to_remote(repo: &Repository, config: &AppConfig) -> Result<(), Error> {
    let mut remote = repo.find_remote(PUSH_REMOTE_NAME)?;

    println!("Pushing to {} from {}", PUSH_REMOTE_NAME, format!("refs/heads/{}", repo.head()?.shorthand().unwrap()));

    let mut remote_callbacks = RemoteCallbacks::new();
    remote_callbacks.credentials(|_, _, _| {
        println!("Attempting to authenticate");
        return git2::Cred::userpass_plaintext("PrMirrorBut", &config.api_token);
    });

    remote_callbacks.push_transfer_progress(|arg1, arg2, arg3| {
        print!("Pushing: {}/{}: {}\r", arg1, arg2, arg3);
        std::io::stdout().flush().unwrap();
    });

    let mut push_options = git2::PushOptions::new();
    push_options.remote_callbacks(remote_callbacks);
    
    // let branch_name = repo.

    remote.push(&[&format!("refs/heads/{}", repo.head()?.shorthand().unwrap())], Some(&mut push_options)).expect("Grb");

    println!("Push successful");

    Ok(())
}

pub fn cherry_pick_commit(repo: &Repository, config: &AppConfig, sha: &str) -> Result<(), Error> {
    {
        let state = RefCell::new(State::default());

        // Handles the progress of the fetch.
        let mut fetch_callback = RemoteCallbacks::new();
        fetch_callback.transfer_progress(|stats| {
            let mut state = state.borrow_mut();
            state.progress = Some(stats.to_owned());
            print(&mut *state);
            let _ = stdout().flush();
            return true;
        });
        
        // Options relating to the fetch.
        let mut fetch_options = FetchOptions::new();
        fetch_options.update_fetchhead(true)
            // .depth(1)
            .remote_callbacks(fetch_callback);

        let mut remote = repo.find_remote(&CLONE_REMOTE_NAME)?;

        remote.fetch(&[&config.clone_repo.branch], Some(&mut fetch_options), None)?;
    }

    let commit = repo.find_commit(git2::Oid::from_str(sha)?)?;

    repo.checkout_index(None, None)?;

    let mut merge_opts = git2::MergeOptions::new();
    merge_opts.fail_on_conflict(false)
        .find_renames(true)
        .standard_style(true)
        .file_favor(FileFavor::Theirs);

    let mut checkout_builder = CheckoutBuilder::new();
    checkout_builder.force()
        .allow_conflicts(true);

    let mut cherrypick_options = git2::CherrypickOptions::new();
    cherrypick_options.checkout_builder(checkout_builder)
        .merge_opts(merge_opts);

    repo.cherrypick(&commit, Some(&mut cherrypick_options))?;

    // repo.merge(&[&commit], Some(&mut merge_opts), Some(&mut checkout_builder))?;
    
    // Commit the changes.
    {
        let signature = Signature::new("PrMirrorBot", "ss14parkstation@simplestation.org", &Time::new(0, 0))?;
        let msg = format!("Cherry-picked commit {} from {}/{}/{}", sha, config.clone_repo.owner, config.clone_repo.name, config.clone_repo.branch);
        let commit = repo.head()?.peel_to_commit()?;
        let tree = repo.find_tree(repo.index()?.write_tree()?)?;
        
        repo.commit(Some("HEAD"), &signature, &signature, &msg, &tree, &[&commit])?;

        repo.cleanup_state()?;
    }

    Ok(())
}

/// Creates and checksout to a new branch with the given name.
pub fn create_branch(repo: &Repository, branch_name: &str) -> Result<(), Error> {
    let head = repo.head()?;
    let head_commit = head.peel_to_commit()?;
    // let head_tree = head_commit.tree()?;
    let branch = repo.branch(branch_name, &head_commit, true)?;
    let branch_ref = branch.into_reference();
    let branch_commit = branch_ref.peel_to_commit()?;
    let branch_tree = branch_commit.tree()?;
    // let diff = repo.diff_tree_to_tree(Some(&head_tree), Some(&branch_tree), None)?;
    let mut opts = CheckoutBuilder::new();
    opts.force();
    repo.checkout_tree(&branch_tree.as_object(), Some(&mut opts))?;
    repo.set_head(&format!("refs/heads/{}", branch_name))?;
    repo.checkout_index(None, Some(&mut opts))?;
    Ok(())
}

/// Returns an up-to-date repo on the required branch.
pub fn ensure_repo(config: &AppConfig) -> Result<Repository, Error> {
    let path = get_repo_path(config);
    let upstream_repo_info = &config.into_repo;
    println!("Repo path: {}", path);

    let repo = match Repository::open(&path) {
        Ok(repo) => {
            println!("Opened existing repo");
            repo
        },
        Err(_) => {
            println!("Failed to open existing repo, setting up new one");
            return setup_new_repo(&config, &path);
        }
    };
    
    let state = RefCell::new(State::default());

    {
        repo.set_head(&format!("refs/heads/{}", upstream_repo_info.branch))?;

        // Handles the progress of the fetch.
        let mut fetch_callback = RemoteCallbacks::new();
        fetch_callback.transfer_progress(|stats| {
            let mut state = state.borrow_mut();
            state.progress = Some(stats.to_owned());
            print(&mut *state);
            let _ = stdout().flush();
            return true;
        });
        
        // Options relating to the fetch.
        let mut fetch_options = FetchOptions::new();
        fetch_options.update_fetchhead(true)
            // .depth(1)
            .remote_callbacks(fetch_callback);

        {
            let mut remote = repo.find_remote(&INTO_REMOTE_NAME)?;
            remote.fetch(&[&upstream_repo_info.branch], Some(&mut fetch_options), None)?;
        }

        let fetch_head = repo.find_reference("FETCH_HEAD")?;
        let remote_commit_ref = repo.reference_to_annotated_commit(&fetch_head)?;

        // Checks if the repo is arleady up to date.
        let analysis = repo.merge_analysis(&[&remote_commit_ref])?;
        if analysis.0.is_up_to_date() {
            drop(fetch_head);
            drop(remote_commit_ref);
            println!("Already up to date");
            return Ok(repo);
        }

        let mut local_branch = repo.find_reference(&format!("refs/heads/{}", upstream_repo_info.branch))?;

        local_branch.set_target(remote_commit_ref.id(), "")?;

        repo.set_head(local_branch.name().unwrap())?;
    }
    
    // Handles the progress of the checkout, and ensuring that the checkout occurs.
    let mut checkout_builder = CheckoutBuilder::new();
    checkout_builder
        .force()
        .overwrite_ignored(true)
        .progress(|path, cur, total| {
        let mut state = state.borrow_mut();
        state.path = path.map(|p| p.to_path_buf());
        state.current = cur;
        state.total = total;
        print(&mut *state);
    });

    repo.checkout_head(Some(&mut checkout_builder))?;
    println!("Checked out head");

    return Ok(repo);
}
    
/// Creates a new repo with all requirements set up.
fn setup_new_repo(config: &AppConfig, path: &String) -> Result<Repository, Error> {
    let upstream_repo_info = &config.into_repo;
    let remote_url = url_from_name(&upstream_repo_info.owner, &upstream_repo_info.name);
    let clone_url = url_from_name(&config.clone_repo.owner, &config.clone_repo.name);
    let owned_url = &config.owned_url;
    
    println!("Remote URL: {}", remote_url);
    
    let state = RefCell::new(State {
        progress: None,
        total: 0,
        current: 0,
        path: None,
        newline: false,
    });
    
    // Handles the progress of the fetch.
    let mut fetch_callback = RemoteCallbacks::new();
    fetch_callback.transfer_progress(|stats| {
            let mut state = state.borrow_mut();
            state.progress = Some(stats.to_owned());
            print(&mut *state);
            let _ = stdout().flush();
            return true;
        });
    
    // Handles the progress of the checkout, and ensuring that the checkout occurs.
    let mut checkout_builder = CheckoutBuilder::new();
    checkout_builder
        .force()
        .progress(|path, cur, total| {
        let mut state = state.borrow_mut();
        state.path = path.map(|p| p.to_path_buf());
        state.current = cur;
        state.total = total;
        print(&mut *state);
    });
    
    // Options relating to the fetch.
    let mut fetch_options = FetchOptions::new();
    fetch_options
        // .depth(1)
        .update_fetchhead(true)
        .remote_callbacks(fetch_callback);

    // Clones the repo.
    let repo = RepoBuilder::new()
        .branch(&upstream_repo_info.branch)
        .fetch_options(fetch_options)
        .with_checkout(checkout_builder)
        .clone(&remote_url, Path::new(&path))?;

    // Adds the required remotes.
    repo.remote(PUSH_REMOTE_NAME, &owned_url)?;
    repo.remote(CLONE_REMOTE_NAME, &clone_url)?;

    println!("Cloned new repo");

    return Ok(repo);
}

pub fn reset_repo(repo: &Repository, config: &AppConfig) -> Result<(), Error> {
    let mut checkout_options = CheckoutBuilder::new();
    checkout_options.force();

    repo.set_head(&format!("refs/heads/{}", config.into_repo.branch))?;
    repo.checkout_head(Some(&mut checkout_options))?;

    // Delete all non-HEAD branches
    for branch in repo.branches(Some(BranchType::Local))? {
        let (mut branch, _) = branch?;
        if branch.is_head() {
            continue;
        }

        let name = branch.name()?.unwrap().to_string();

        match branch.delete() {
            Ok(_) => println!("Cleaned up branch {}", name),
            Err(_) => println!("Failed to delete branch {}", name),
        }
    }

    Ok(())
}

// fn get_checkout_options(state: &mut State) -> CheckoutBuilder<'static> {
//     let mut checkout_builder = CheckoutBuilder::new();
//     checkout_builder.force();
//     checkout_builder.progress(|path, cur, total| {
//         state.path = path.map(|p| p.to_path_buf());
//         state.current = cur;
//         state.total = total;
//         print(state);
//     });
//     return checkout_builder;
// }

fn get_repo_path(config: &AppConfig) -> String {
    let path = format!("{}_{}_cl_{}_{}", config.clone_repo.owner, config.clone_repo.name, config.into_repo.owner, config.into_repo.name);
    return path;
}

// Copied from the example docs, just prints a Git-style progress bar when cloning or fetching.
fn print(state: &mut State) {
    let stats = state.progress.as_ref();
    if stats.is_none() {
        print!(
            "Receiving objects: {:4}/{:4} {}\r",
            state.current, state.total, state
                .path
                .as_ref()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        io::stdout().flush().unwrap();
        return;
    }
    let stats = stats.unwrap();
    let network_pct = (100 * stats.received_objects()) / stats.total_objects();
    let index_pct = (100 * stats.indexed_objects()) / stats.total_objects();
    let co_pct = if state.total > 0 {
        (100 * state.current) / state.total
    } else {
        0
    };
    let kbytes = stats.received_bytes() / 1024;
    if stats.received_objects() == stats.total_objects() {
        if !state.newline {
            println!();
            state.newline = true;
        }
        print!(
            "Resolving deltas {}/{}\r",
            stats.indexed_deltas(),
            stats.total_deltas()
        );
    } else {
        print!(
            "net {:3}% ({:4} kb, {:5}/{:5})  /  idx {:3}% ({:5}/{:5})  \
            /  chk {:3}% ({:4}/{:4}) {}\r",
            network_pct,
            kbytes,
            stats.received_objects(),
            stats.total_objects(),
            index_pct,
            stats.indexed_objects(),
            stats.total_objects(),
            co_pct,
            state.current,
            state.total,
            state
                .path
                .as_ref()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        )
    }
    io::stdout().flush().unwrap();
}

fn url_from_name(owner: &str, name: &str) -> String {
    return format!("https://github.com/{}/{}", owner, name);
}

#[derive(Default)]
struct State {
    progress: Option<Progress<'static>>,
    total: usize,
    current: usize,
    path: Option<PathBuf>,
    newline: bool,
}