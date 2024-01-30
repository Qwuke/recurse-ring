use addr::parse_domain_name;
use anyhow::{anyhow, Context, Error, Result};
use octocrab::{
    models::repos::{CommitAuthor, FileUpdate},
    Octocrab,
};
use rocket::{
    form,
    form::Form,
    http::{
        uri::{Absolute, Uri},
        Status,
    },
    response::{Debug, Redirect},
    State,
};
use std::collections::BTreeMap;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    get_deserialized_sites, get_named_sites, get_site_content, ClientTokens, SiteData, SitesMap,
    User, GH_REPO, GH_SITES_PATH, GH_USER,
};

const GH_COMMITTER_NAME: &str = "Webring Bot";
const GH_COMMITTER_EMAIL: &str = "recurseringbot@server.fake";

#[derive(FromForm)]
pub struct WebsiteSignup {
    name: String,
    #[field(validate = valid_domain())]
    url: String,
    #[form(default = "false")]
    is_anonymous: bool,
}

fn valid_domain<'v>(input_url: &str) -> form::Result<'v, ()> {
    let unparseable_domain_message = "Please use a parseable domain name";
    let parsed_uri = Uri::parse::<Absolute>(input_url)
        .map_err(|_| form::Error::validation(unparseable_domain_message))?;

    let invalid_domain_message = "Please use a valid domain name";
    let host_domain = parsed_uri
        .absolute()
        .ok_or_else(|| form::Error::validation(invalid_domain_message))?
        .authority()
        .ok_or_else(|| form::Error::validation(invalid_domain_message))?
        .host();
    let domain = parse_domain_name(host_domain)
        .map_err(|_| form::Error::validation(unparseable_domain_message))?;

    if !domain.has_known_suffix() {
        Err(form::Error::validation(invalid_domain_message))?;
    }

    Ok(())
}

async fn get_sites_and_shas(gh_client: &Octocrab) -> Result<(Vec<SiteData>, String)> {
    let sites_sha = get_site_content(&gh_client).await?.sha;
    let recurse_sites = get_deserialized_sites(&gh_client).await?;
    Ok((recurse_sites, sites_sha))
}

async fn update_and_build_formatted_sites(
    sites_data: &SitesMap,
    client_tokens: &Mutex<ClientTokens>,
    recurse_sites: Vec<SiteData>,
) -> Result<String, Error> {
    let json_output = serde_json::to_string_pretty::<Vec<SiteData>>(&recurse_sites)
        .context("Could not prettify serialized sites")?;

    let mut writeable_sites = sites_data.write().await;
    writeable_sites.clear();
    let recurse_secret = &client_tokens.lock().await.recurse_secret;
    let sites_with_names = get_named_sites(recurse_sites, recurse_secret).await?;
    writeable_sites.append(
        &mut sites_with_names
            .into_iter()
            .map(|site| (site.website_id, site))
            .collect::<BTreeMap<u32, SiteData>>());
    Ok(json_output)
}

async fn update_gh_sites(
    locked_gh_client: &Octocrab,
    json_output: String,
    sites_sha: String,
    commit_message: String,
) -> Result<FileUpdate, Error> {
    let commit_author = CommitAuthor {
        name: GH_COMMITTER_NAME.to_string(),
        email: GH_COMMITTER_EMAIL.to_string(),
    };
    locked_gh_client
        .repos(GH_USER, GH_REPO)
        .update_file(GH_SITES_PATH, commit_message, json_output, &sites_sha)
        .branch("main")
        .commiter(commit_author.clone())
        .author(commit_author)
        .send().await
        .context("Could not update GitHub site files")
}

#[post("/sites/add", data = "<form>")]
pub async fn add(
    user: User,
    sites_data: &State<SitesMap>,
    form: Form<WebsiteSignup>,
    gh_client: &State<Mutex<Octocrab>>,
    client_tokens: &State<Mutex<ClientTokens>>,
) -> Result<Redirect, Debug<Error>> {
    let locked_gh_client = gh_client.lock().await;
    let (mut recurse_sites, sites_sha) = get_sites_and_shas(&locked_gh_client).await?;

    let max_website_id = recurse_sites
        .iter()
        .max_by_key(|site| site.website_id)
        .expect("Should always have an maxium key")
        .website_id;
    let uuid_str = Uuid::new_v4().to_string();
    let new_site = SiteData {
        website_id: max_website_id + 1,
        website_uuid: uuid_str.clone(),
        recurse_id: user.id,
        website_name: form.name.clone(),
        recurse_name: None,
        is_anonymous: form.is_anonymous,
        url: form.url.clone(),
    };
    recurse_sites.push(new_site);

    let json_output =
        update_and_build_formatted_sites(sites_data, client_tokens, recurse_sites).await?;

    let commit_message = format!("Automated Action: Added a new site for '{}'", form.url);
    update_gh_sites(&locked_gh_client, json_output, sites_sha, commit_message).await?;

    Ok(Redirect::to(format!("/?id={}&uuid_str={}", max_website_id + 1, uuid_str)))
}

#[post("/sites/update/<id>", data = "<form>")]
pub async fn update(
    user: User,
    sites_data: &State<SitesMap>,
    form: Form<WebsiteSignup>,
    id: u32,
    gh_client: &State<Mutex<Octocrab>>,
    client_tokens: &State<Mutex<ClientTokens>>,
) -> Result<Redirect, (Status, Debug<Error>)> {
    let locked_gh_client = gh_client.lock().await;
    let (mut recurse_sites, sites_sha) 
        = get_sites_and_shas(&locked_gh_client).await
            .map_err(|e| (Status::InternalServerError, Debug(e)))?;

    let update_index = recurse_sites
        .iter()
        .position(|site| site.website_id == id)
        .context("Could not find a site with that id")
        .map_err(|e| (Status::NotFound, Debug(e)))?;
    
    let current_site = recurse_sites.remove(update_index);
    if current_site.recurse_id != user.id {
        return Err((
            Status::Unauthorized,
            Debug(anyhow!("Access to this resource is forbidden")),
        ));
    }

    let updated_site = SiteData {
        website_name: form.name.clone(),
        url: form.url.clone(),
        is_anonymous: form.is_anonymous,
        website_id: current_site.website_id,
        website_uuid: current_site.website_uuid,
        recurse_id: current_site.recurse_id,
        recurse_name: current_site.recurse_name,
    };
    recurse_sites.push(updated_site);
    recurse_sites.sort_by(|a, b| {
        a.website_id
            .partial_cmp(&b.website_id)
            .expect("Ordering should exist")
    });

    let json_output = update_and_build_formatted_sites(sites_data, client_tokens, recurse_sites).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;

    let commit_message = format!(
        "Automated Action: Updated a site for '{}'",
        current_site.url
    );
    update_gh_sites(&locked_gh_client, json_output, sites_sha, commit_message).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;

    Ok(Redirect::to(format!("/")))
}

#[get("/sites/delete/<id>")]
pub async fn delete(
    user: User,
    sites_data: &State<SitesMap>,
    id: u32,
    gh_client: &State<Mutex<Octocrab>>,
    client_tokens: &State<Mutex<ClientTokens>>,
) -> Result<Redirect, (Status, Debug<Error>)> {
    let locked_gh_client = gh_client.lock().await;
    let (mut recurse_sites, sites_sha) = get_sites_and_shas(&locked_gh_client).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;

    let removal_index = recurse_sites
        .iter()
        .position(|site| site.website_id == id)
        .context("Could not find a site with that id")
        .map_err(|e| (Status::NotFound, Debug(e)))?;
    let removed_site = recurse_sites.remove(removal_index);
    if removed_site.recurse_id != user.id {
        return Err((
            Status::Unauthorized,
            Debug(anyhow!("Access to this resource is forbidden")),
        ));
    }

    let json_output = update_and_build_formatted_sites(sites_data, client_tokens, recurse_sites).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;

    let commit_message = format!(
        "Automated Action: Removed a site for '{}'",
        removed_site.url
    );
    update_gh_sites(&locked_gh_client, json_output, sites_sha, commit_message).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;

    Ok(Redirect::to(format!("/")))
}
