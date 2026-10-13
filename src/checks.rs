use anyhow::{anyhow, Context, Error, Result};
use octocrab::Octocrab;
use regex::RegexSet;
use reqwest::Client;
use rocket::{
    futures::future::join_all,
    response::{Debug, Redirect},
    serde::{Deserialize, Serialize},
    State,
};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use crate::{get_gh_content, sites::update_gh_file, SiteData, SitesMap, User};

const GH_CHECKS_PATH: &str = "checks.json";
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
const MAX_PAGE_BYTES: usize = 500 * 1024;

// Each required link counts if either its HTML id or its ring.recurse.com href is on the page
const RING_LINK_PATTERNS: [&str; 3] = [
    r"rc-ring-prev|ring\.recurse\.com/prev",
    r#"rc-ring-home|ring\.recurse\.com/?["'\s>]"#,
    r"rc-ring-next|ring\.recurse\.com/next",
];

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct CheckLog {
    checked_at: u64,
    statuses: BTreeMap<u32, String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time should be after the Unix epoch")
        .as_secs()
}

pub async fn get_check_log(octocrab: &Octocrab) -> Result<CheckLog> {
    let decoded_log = get_gh_content(octocrab, GH_CHECKS_PATH).await?
        .decoded_content()
        .ok_or(anyhow!("Could not decode GitHub content"))?;

    serde_json::from_str(&decoded_log).context("Unable to deserialize check log from GitHub file")
}

// Returns the first MAX_PAGE_BYTES of a page, and whether the page was any larger than that
async fn download_page(client: &Client, url: &str) -> reqwest::Result<(String, bool)> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut page = Vec::new();

    while let Some(chunk) = response.chunk().await? {
        page.extend_from_slice(&chunk);
        if page.len() > MAX_PAGE_BYTES {
            page.truncate(MAX_PAGE_BYTES);
            return Ok((String::from_utf8_lossy(&page).into_owned(), true));
        }
    }

    Ok((String::from_utf8_lossy(&page).into_owned(), false))
}

async fn check_site(client: &Client, ring_links: &RegexSet, url: &str) -> String {
    match download_page(client, url).await {
        Ok((page, _)) if ring_links.matches(&page).iter().count() == ring_links.len() => "Compliant",
        Ok((_, true)) => "Page Too Big",
        Ok((_, false)) => "(Possibly) Non-Compliant",
        Err(_) => "Error Downloading",
    }.to_string()
}

#[post("/sites/check")]
pub async fn check(
    _user: User,
    sites_data: &State<SitesMap>,
    check_log: &State<Mutex<CheckLog>>,
    gh_client: &State<Mutex<Octocrab>>,
) -> Result<Redirect, Debug<Error>> {
    // The check log stays locked while a check is in progress, so only one check runs at a time
    let Ok(mut locked_check_log) = check_log.try_lock() else {
        return Ok(Redirect::to("/"));
    };
    if now() < locked_check_log.checked_at + CHECK_INTERVAL_SECS {
        return Ok(Redirect::to("/"));
    }

    let recurse_sites = sites_data
        .read().await
        .values()
        .cloned()
        .collect::<Vec<SiteData>>();
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Recurse Webring Bot (https://ring.recurse.com)")
        .build()
        .context("Unable to build reqwest client")?;
    let ring_links = RegexSet::new(RING_LINK_PATTERNS).context("Invalid ring link patterns")?;

    let statuses = join_all(recurse_sites
        .iter()
        .map(|site| check_site(&client, &ring_links, &site.url))).await;

    *locked_check_log = CheckLog {
        checked_at: now(),
        statuses: recurse_sites
            .iter()
            .map(|site| site.website_id)
            .zip(statuses)
            .collect(),
    };

    let json_output = serde_json::to_string_pretty(&*locked_check_log)
        .context("Could not prettify serialized check log")?;
    let locked_gh_client = gh_client.lock().await;
    let checks_sha = get_gh_content(&locked_gh_client, GH_CHECKS_PATH).await?.sha;
    let commit_message = "Automated Action: Checked all sites for webring links".to_string();
    update_gh_file(&locked_gh_client, GH_CHECKS_PATH, json_output, checks_sha, commit_message).await?;

    Ok(Redirect::to("/"))
}
