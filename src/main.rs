#![feature(btree_cursors)]
use rocket::{
    http::{Cookie, CookieJar, Status, uri::{Uri, Absolute}},
    form::Form,
    request::{FromRequest, Outcome},
    response::{Redirect, Debug},
    fs::{FileServer, relative},
    serde::{Serialize, Deserialize, json::Json}, 
    State, request, form, Config};
use tokio::sync::{Mutex, MutexGuard, RwLock};
use std::collections::BTreeMap;
use std::ops::Bound;
use anyhow::{Error, Result, anyhow, Context};
use reqwest::Client;
use rocket_dyn_templates::{Template, context};
use figment::{Figment, providers::Env};
use octocrab::{Octocrab, models::repos::{CommitAuthor, Content, FileUpdate}};
use addr::parse_domain_name;
use rand::{thread_rng, Rng};
use uuid::Uuid;

mod oauth;

#[macro_use] extern crate rocket;

const GH_USER: &str = "Qwuke";
const GH_REPO: &str = "recurse-ring";
const GH_SITES_PATH: &str = "sites.json";
const GH_COMMITTER_NAME: &str = "Webring Bot";
const GH_COMMITTER_EMAIL: &str = "recurseringbot@server.fake";

const RECURSE_BASE_URL: &str = "https://www.recurse.com/api/v1/";
const _RECURSE_OAUTH_URL: &str = "https://www.recurse.com/oauth/authorize";
const _RECURSE_TOKEN_URL: &str = "https://www.recurse.com/oauth/token";

#[derive(Deserialize)]
struct ClientTokens {
    recurse_client_id: String,
    recurse_secret: String,
    github_secret: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct SiteData {
    website_id: u32,
    website_uuid: String,
    recurse_id: u32,
    website_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    recurse_name: Option<String>,
    url: String,
}

#[derive(FromForm)]
struct WebsiteSignup {
    name: String,
    #[field(validate = valid_domain())]
    url: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct User {
    pub id: u32,
    pub name: String,
    pub token: Option<String>,
}

type SitesMap = RwLock<BTreeMap<u32, SiteData>>;

#[rocket::async_trait]
impl<'r> FromRequest<'r> for User {
    type Error = ();

    async fn from_request(request: &'r request::Request<'_>) -> Outcome<User, ()> {
        let cookies = request
            .guard::<&CookieJar<'_>>()
            .await
            .expect("Cookies should be accessible from the request");

        match (cookies.get_private("name"), cookies.get_private("id"), cookies.get_private("api_token")) {
            (Some(name), Some(id), Some(token)) => 
                Outcome::Success(User {
                    id: id.value().parse::<u32>().unwrap(),
                    name: name.value().to_string(),
                    token: Some(token.value().to_string()) }),
            _ => Outcome::Forward(Status::Unauthorized)
        }
    }
}

fn valid_domain<'v>(input_url: &str) -> form::Result<'v, ()> {
    let unparseable_domain_message = "Please use a parseable domain name";
    let parsed_uri = Uri::parse::<Absolute>(input_url)
        .map_err(|_| form::Error::validation(unparseable_domain_message))?;

    let invalid_domain_message = "Please use a valid domain name";
    let host_domain = parsed_uri
        .absolute()
        .ok_or_else(|| { form::Error::validation(invalid_domain_message) })?
        .authority()
        .ok_or_else(|| { form::Error::validation(invalid_domain_message) })?
        .host();
    let domain = parse_domain_name(host_domain)
        .map_err(|_| form::Error::validation(unparseable_domain_message))?;
    
    if !domain.has_known_suffix() {
        Err(form::Error::validation(invalid_domain_message))?;
    }

    Ok(())
}

async fn get_site_content(octocrab: &Octocrab) -> Result<Content> {
    let all_contents = octocrab.repos(GH_USER, GH_REPO)
        .get_content()
        .path(GH_SITES_PATH)
        .r#ref("main")
        .send().await?;

    all_contents.items.into_iter()
        .next()
        .ok_or(anyhow!("Missing GitHub content for {}", GH_SITES_PATH))
}

async fn get_deserialized_sites(octocrab: &Octocrab) -> Result<Vec<SiteData>> {
    let decoded_sites = get_site_content(octocrab).await?
        .decoded_content()
        .ok_or(anyhow!("Could not decode GitHub content"))?;

    serde_json::from_str(&decoded_sites)
        .context("Unable to deserialize sites from GitHub file")
}

async fn get_named_sites(unnamed_sites: Vec<SiteData>, bearer_token: &String) -> Result<Vec<SiteData>> {
    let mut sites_with_names = Vec::new();

    for site in unnamed_sites {
        if site.website_id != 0 {
            let response = Client::new()
                .get(format!("{}profiles/{}", RECURSE_BASE_URL, site.recurse_id))
                .bearer_auth(bearer_token)
                .send().await?;
            let res = response.text().await?;
            let Ok(user) = serde_json::from_str::<User>(res.as_str()) else {
                return Ok(Vec::new());
            };

            sites_with_names.push(SiteData { 
                website_id: site.website_id,
                website_uuid: site.website_uuid,
                recurse_id: site.recurse_id,
                website_name: site.website_name,
                recurse_name: Some(user.name),
                url: site.url,
            });
        }
    }

    Ok(sites_with_names)
}

async fn get_sites_and_shas(gh_client: &Octocrab) -> Result<(Vec<SiteData>, String)> {
    let sites_sha = get_site_content(&gh_client).await?.sha;
    let recurse_sites = get_deserialized_sites(&gh_client).await?;
    Ok((recurse_sites, sites_sha))
}

async fn update_and_build_formatted_sites(sites_data: &SitesMap, client_tokens: &Mutex<ClientTokens>, recurse_sites: Vec<SiteData>) -> Result<String, Error> {
    let json_output = serde_json::to_string_pretty::<Vec<SiteData>>(&recurse_sites)
        .context("Could not prettify serialized sites")?;
    
    let mut writeable_sites = sites_data.write().await;
    writeable_sites.clear();
    let recurse_secret = &client_tokens.lock().await.recurse_secret;
    let sites_with_names = get_named_sites(recurse_sites, recurse_secret).await?;
    writeable_sites.append(&mut sites_with_names.into_iter()
        .map(|site| (site.website_id, site))
        .collect::<BTreeMap<u32, SiteData>>());
    Ok(json_output)
}

async fn update_gh_sites(locked_gh_client: &Octocrab, json_output: String, sites_sha: String, commit_message: String) -> Result<FileUpdate, Error> {
    let commit_author = CommitAuthor {
        name: GH_COMMITTER_NAME.to_string(),
        email: GH_COMMITTER_EMAIL.to_string() 
    };  
    locked_gh_client.repos(GH_USER, GH_REPO)
        .update_file(
            GH_SITES_PATH,
            commit_message,
            json_output,
            &sites_sha)
        .branch("main")
        .commiter(commit_author.clone())
        .author(commit_author)
        .send().await
        .context("Could not update GitHub site files")
}

#[get("/?<id>&<uuid_str>")]
async fn authed(user: User, sites_data: &State<SitesMap>, 
    id: Option<u32>, uuid_str: Option<String>) -> Template {
    
    let recurse_sites = sites_data.read().await
        .values().cloned()
        .collect::<Vec<SiteData>>();

    Template::render("index", context! { sites: recurse_sites, user, id, uuid_str })
}

#[get("/", rank = 2)]
async fn home(sites_data: &State<SitesMap>) -> Template {
    let recurse_sites = sites_data.read().await
        .values().cloned()
        .collect::<Vec<SiteData>>();

    Template::render("index", context! { sites: recurse_sites })
}

#[post("/sites/add", data = "<form>")]
async fn add(user: User, sites_data: &State<SitesMap>, form: Form<WebsiteSignup>, 
    gh_client: &State<Mutex<Octocrab>>, client_tokens: &State<Mutex<ClientTokens>>) 
        -> Result<Redirect, Debug<Error>> {   
    let locked_gh_client = gh_client.lock().await;
    let (mut recurse_sites, sites_sha) = get_sites_and_shas(&locked_gh_client).await?;
    
    let max_website_id = recurse_sites.iter()
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
        url: form.url.clone(), 
    };
    recurse_sites.push(new_site);

    let json_output = update_and_build_formatted_sites(sites_data, client_tokens, recurse_sites).await?;
    
    let commit_message = format!("Automated Action: Added a new site for '{}'", form.url);
    update_gh_sites(&locked_gh_client, json_output, sites_sha, commit_message).await?;

    Ok(Redirect::to(format!("/?id={}&uuid_str={}", max_website_id + 1, uuid_str)))
}

#[put("/sites/update/<id>", data = "<form>")]
async fn update(user: User, sites_data: &State<SitesMap>, form: Form<WebsiteSignup>, id: u32,
    gh_client: &State<Mutex<Octocrab>>, client_tokens: &State<Mutex<ClientTokens>>) 
        -> Result<Redirect, (Status, Debug<Error>)> {   
    let locked_gh_client = gh_client.lock().await;
    let (mut recurse_sites, sites_sha) = get_sites_and_shas(&locked_gh_client).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;

    let update_index = recurse_sites.iter()
        .position(|site| site.website_id == id)
        .context("Could not find a site with that id")
        .map_err(|e| (Status::NotFound, Debug(e)))?;
    let current_site = recurse_sites.remove(update_index);
    if current_site.recurse_id != user.id {
        return Err((Status::Unauthorized, Debug(anyhow!("Access to this resource is forbidden")))); 
    }
    let updated_site = SiteData {
        website_name: form.name.clone(),
        url: form.url.clone(),
        ..current_site
    };
    recurse_sites.push(updated_site);
    recurse_sites.sort_by(|a, b| a.website_id.partial_cmp(&b.website_id).expect("Ordering should exist"));

    let json_output = update_and_build_formatted_sites(sites_data, client_tokens, recurse_sites).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;

    let commit_message = format!("Automated Action: Updated a site for '{}'", current_site.url);
    update_gh_sites(&locked_gh_client, json_output, sites_sha, commit_message).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;
    
    Ok(Redirect::to(format!("/")))
}

#[delete("/sites/delete/<id>")]
async fn delete(user: User, sites_data: &State<SitesMap>, id: u32, 
    gh_client: &State<Mutex<Octocrab>>, client_tokens: &State<Mutex<ClientTokens>>) 
        -> Result<Redirect, (Status, Debug<Error>)> {
    let locked_gh_client = gh_client.lock().await;   
    let (mut recurse_sites, sites_sha) = get_sites_and_shas(&locked_gh_client).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;
    
    let removal_index = recurse_sites.iter()
        .position(|site| site.website_id == id)
        .context("Could not find a site with that id")
        .map_err(|e| (Status::NotFound, Debug(e)))?;
    let removed_site = recurse_sites.remove(removal_index);
    if removed_site.recurse_id != user.id {
        return Err((Status::Unauthorized, Debug(anyhow!("Access to this resource is forbidden")))); 
    }

    let json_output = serde_json::to_string_pretty::<Vec<SiteData>>(&recurse_sites)
        .context("Could not prettify serialized sites")
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;

    let commit_message = format!("Automated Action: Removed a site for '{}'", removed_site.url);
    update_gh_sites(&locked_gh_client, json_output, sites_sha, commit_message).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;

    Ok(Redirect::to(format!("/")))
}

#[get("/auth/logout")]
fn logout(_user: User, cookies: &CookieJar<'_>) -> Redirect {
    cookies.remove(Cookie::from("name"));
    cookies.remove(Cookie::from("id"));
    cookies.remove(Cookie::from("api_token"));
    Redirect::to("/")
}

#[get("/prev?<requested_id>")]
async fn prev(requested_id: u32, sites_data: &State<SitesMap>) -> Redirect {
    let readable_sites = sites_data.read().await;
    let last_key_value = readable_sites.last_key_value()
                .expect("Should always have the last site to wrap around to");
    let (_actual_id, prev_site) = readable_sites.lower_bound(Bound::Included(&requested_id))
        .key_value()
        .unwrap_or(last_key_value);
        
    Redirect::to(prev_site.url.clone())
}

#[get("/next?<requested_id>")]
async fn next(requested_id: u32, sites_data: &State<SitesMap>) -> Redirect {
    let readable_sites = sites_data.read().await;
    let first_key_value = readable_sites.first_key_value()
                .expect("Should always have the first site to wrap around to");
    let (_actual_id, next_site) = readable_sites.upper_bound(Bound::Included(&requested_id))
        .key_value()
        .unwrap_or(first_key_value);
        
    Redirect::to(next_site.url.clone())
}

#[get("/rand")]
async fn random(sites_data: &State<SitesMap>) -> Redirect {
    let readable_sites = sites_data.read().await;
    let mut rng = thread_rng();
    let non_main_site_index = rng.gen_range(1..readable_sites.len());
    let random_site = readable_sites.values().nth(non_main_site_index)
        .expect("Should always have a random indice based on length");
    Redirect::to(random_site.url.clone())
}

#[get("/sites.json")]
async fn dynamic_json(sites_data: &State<SitesMap>) -> Json<Vec<SiteData>> {
    let serializable_sites = sites_data.read().await
        .values().cloned()
        .map(|mut site| { site.recurse_name = None; site })
        .collect::<Vec<SiteData>>();
    Json(serializable_sites)
}

#[get("/health")]
fn health() -> String { "pong!".to_owned() }

#[launch]
#[tokio::main]
async fn rocket() -> _ {
    let config: ClientTokens = Figment::from(Env::prefixed("CONFIG_"))
        .extract()
        .expect("Config should be extractable");

    let octocrab = Octocrab::builder()
        .personal_token(config.github_secret.clone())
        .build()
        .expect("GitHub client should build using personal token");

    let initial_site_data = get_deserialized_sites(&octocrab).await
        .expect("Should retrieve initial sites from GitHub");

    let sites_with_names = get_named_sites(initial_site_data, &config.recurse_secret).await
        .expect("Should retrieve names of site owners from Recurse");

    let ordered_sites = sites_with_names.into_iter()
        .map(|site| (site.website_id, site))
        .collect::<BTreeMap<u32, SiteData>>();

    rocket::build()
        .manage(Mutex::new(octocrab))
        .manage(Mutex::new(config))
        .manage(RwLock::new(ordered_sites) as SitesMap)
        .attach(oauth::recurse_oauth_fairing())
        .attach(Template::fairing())
        .configure(Config::figment().merge(("port", 4000)))
        .mount("/", routes![authed, home, add, update, delete, logout, prev,
            next, random, dynamic_json, health])
        .mount("/", FileServer::from(relative!("static")))
        .mount("/static/", FileServer::from(relative!("static")).rank(3))
}
