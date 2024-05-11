use octocrab::models::{pulls::PullRequest, repos::RepoCommit};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrTemplate {
    title: String,
    original_desc: String,
    number: String,
    labels: Vec<String>,
    merge_sha: String,
    changed_files: String,
    additions: String,
    deletions: String,
    url_pr: String,
    url_diff: String,
    url_commits: String,
    url_comments: String,
    owner_name: String,
    owner_link: String,
    owner_icon: String,
    repo_name: String,
    repo_link: String,
    license: String,
    open_user_name: String,
    open_user_link: String,
    open_user_icon: String,
    merge_user_name: String,
    merge_user_link: String,
    merge_user_icon: String,
    open_date: String,
    merge_date: String,
}

impl PrTemplate {
    pub fn new(pr: &PullRequest, merge_commit: Option<RepoCommit>) -> Self {
        let pr = pr.clone();
        let mut template = PrTemplate::default();

        if pr.title.is_some() {
            template.title = pr.title.unwrap();
        }

        if pr.body.is_some() {
            template.original_desc = pr.body.unwrap();
        }

        template.number = pr.number.to_string();

        if pr.labels.is_some() {
            for label in pr.labels.unwrap().iter() {
                template.labels.push(label.name.clone());
            }
        }

        if pr.merge_commit_sha.is_some() {
            template.merge_sha = pr.merge_commit_sha.unwrap();
        }

        if pr.html_url.is_some() {
            template.url_pr = pr.html_url.unwrap().as_str().to_string();
        }

        if pr.diff_url.is_some() {
            template.url_diff = pr.diff_url.unwrap().as_str().to_string();
        }

        if pr.commits_url.is_some() {
            template.url_commits = pr.commits_url.unwrap().as_str().to_string();
        }

        if pr.comments_url.is_some() {
            template.url_comments = pr.comments_url.unwrap().as_str().to_string();
        }

        if pr.user.is_some() {
            let user = &pr.user.unwrap();
            template.open_user_name = user.login.clone();
            template.open_user_link = user.html_url.as_str().to_string();
            template.open_user_icon = user.avatar_url.as_str().to_string();
        }

        if pr.created_at.is_some() {
            template.open_date = pr.created_at.unwrap().to_string();
        }

        if pr.merged_at.is_some() {
            template.merge_date = pr.merged_at.unwrap().to_string();
        }

        if pr.base.repo.is_some() {
            let repo = &pr.base.repo.unwrap();
            if repo.owner.is_some() {
                let owner = &repo.owner.clone().unwrap();
                template.owner_name = owner.login.clone();
                template.owner_link = owner.html_url.as_str().to_string();
                template.owner_icon = owner.avatar_url.as_str().to_string();
            }

            if repo.html_url.is_some() {
                template.repo_link = repo.html_url.clone().unwrap().as_str().to_string();

                if repo.license.is_some() {
                    template.license = repo.license.clone().unwrap().name.clone();
                }
            }

            template.repo_name = repo.name.clone();
        }

        if merge_commit.is_some() {
            let commit = merge_commit.unwrap();

            if let Some(user) = commit.committer {
                //TODO: Verify this is the correct person. Should be?
                template.merge_user_name = user.login.clone();
                template.merge_user_link = user.html_url.as_str().to_string();
                template.merge_user_icon = user.avatar_url.as_str().to_string();
            }

            if let Some(files) = commit.files {
                template.changed_files = files.len().to_string();
            }

            if let Some(stats) = &commit.stats {
                template.additions = stats.additions.unwrap_or_default().to_string();
            }
            if let Some(stats) = &commit.stats {
                template.deletions = stats.deletions.unwrap_or_default().to_string();
            }
        }

        return template;
    }

    pub fn to_markdown(&self) -> String {
        return format!(
            "## Mirror of  PR #{number}: [{title}]({url_pr}) from <img src=\"{owner_icon}\" alt=\"{owner_name}\" width=\"22\"/> [{owner_name}]({owner_link})/[{repo_name}]({repo_link})\n\
            \n\
            ###### `{merge_sha}`\n\
            \n\
            PR opened by <img src=\"{open_user_icon}\" width=\"16\"/><a href=\"{open_user_link}\"> {open_user_name}</a> at {open_date} - merged at {merge_date}\n\
            \n\
            ---\n\
            \n\
            PR changed {changed_files} files with {additions} additions and {deletions} deletions.\n\
            \n\
            The PR had the following labels:\n\
            {labels_list}\n\
            \n\
            ---\n\
            \n\
            <details open=\"true\"><summary><h1>Original Body</h1></summary>\n\
            \n\
            {original_desc}\n\
            \n\
            </details>",
            
            number=self.number,
            title=self.title,
            url_pr=self.url_pr,
            owner_icon=self.owner_icon,
            owner_name=self.owner_name,
            owner_link=self.owner_link,
            repo_name=self.repo_name,
            repo_link=self.repo_link,
            open_user_icon=self.open_user_icon,
            open_user_name=self.open_user_name,
            open_user_link=self.open_user_link,
            open_date=self.open_date,
            //-- merge_user_icon=self.merge_user_icon,
            //-- merge_user_name=self.merge_user_name,
            //-- merge_user_link=self.merge_user_link,
            merge_date=self.merge_date,
            //-- PR merged by <img src=\"{merge_user_icon}\" width=\"16\"/><a href=\"{merge_user_link}\"> {merge_user_name}</a> at {merge_date}\n\
            // Turns out the 'author' of the PR is always GitHub webflow... :T
            merge_sha=self.merge_sha,
            original_desc=self.original_desc.split("\n").into_iter().map(|l| format!("> {}\n", l)).collect::<String>(),
            labels_list=self.labels.iter().map(|l| format!("- {}\n", l)).collect::<String>(),
            changed_files=self.changed_files,
            additions=self.additions,
            deletions=self.deletions,
        );
    }
}

impl Default for PrTemplate {
    fn default() -> Self {
        PrTemplate {
            title: String::new(),
            original_desc: String::new(),
            number: "???".to_string(),
            labels: Vec::new(),
            merge_sha: String::new(),
            changed_files: String::new(),
            additions: String::new(),
            deletions: String::new(),
            url_pr: String::new(),
            url_diff: String::new(),
            url_commits: String::new(),
            url_comments: String::new(),
            owner_name: String::new(),
            owner_link: String::new(),
            owner_icon: String::new(),
            repo_name: String::new(),
            repo_link: String::new(),
            license: String::new(),
            open_user_name: "Unknown".to_string(),
            open_user_link: String::new(),
            open_user_icon: String::new(),
            merge_user_name: "Unknown".to_string(),
            merge_user_link: String::new(),
            merge_user_icon: String::new(),
            open_date: String::new(),
            merge_date: String::new(),
        }
    }
}
