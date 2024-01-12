#![feature(btree_cursors)]
use rocket::{
    http::{Cookie, CookieJar, Status, uri::{Uri, Absolute}},
    form::Form,
    request::{FromRequest, Outcome},
    response::{Redirect, Debug},
    fs::{FileServer, NamedFile, relative},
    serde::{Serialize, Deserialize, json::Json}, 
    State, request, form, Config};
use tokio::sync::{Mutex, RwLock};
use std::collections::BTreeMap;
use std::ops::Bound;
use anyhow::{Error, Result, anyhow, Context};
use reqwest::Client;
use rocket_dyn_templates::{Template, context};
use figment::{Figment, providers::Env};
use octocrab::{Octocrab, models::repos::{CommitAuthor, Content}};
use addr::parse_domain_name;
use rand::{thread_rng, Rng};
use uuid::Uuid;
use std::path::Path;

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

#[get("/?<id>&<uuid_str>")]
async fn authed(user: User, sites_data: &State<SitesMap>, id: Option<u32>, uuid_str: Option<String>) 
    -> Result<Template, Debug<Error>> {
    
    let recurse_sites = sites_data.read().await
        .values().cloned()
        .collect::<Vec<SiteData>>();
    let sites_with_names = get_named_sites(recurse_sites, user.token.as_ref().unwrap()).await?;

    Ok(Template::render("index", context! { sites: sites_with_names, user, id, uuid_str }))
}

#[get("/", rank = 2)]
async fn home(sites_data: &State<SitesMap>, client_tokens: &State<Mutex<ClientTokens>>) 
    -> Result<Template, Debug<Error>> {
    
    let recurse_sites = sites_data.read().await
        .values().cloned()
        .collect::<Vec<SiteData>>();
    let sites_with_names = get_named_sites(recurse_sites, &client_tokens.lock().await.recurse_secret).await?;

    Ok(Template::render("index", context! { sites: sites_with_names }))
}

#[post("/auth/add", data = "<form>")]
async fn add(user: User, sites_data: &State<SitesMap>, form: Form<WebsiteSignup>, gh_client: &State<Mutex<Octocrab>>) 
    -> Result<Redirect, Debug<Error>> {
   
    let locked_gh_client = gh_client.lock().await;
    let sites_sha = get_site_content(&locked_gh_client).await?.sha;
    let mut recurse_sites = get_deserialized_sites(&locked_gh_client).await?;
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
    let json_output = serde_json::to_string_pretty::<Vec<SiteData>>(&recurse_sites)
        .context("Could not prettify serialized sites")?;
    let commit_author = CommitAuthor {
        name: GH_COMMITTER_NAME.to_string(),
        email: GH_COMMITTER_EMAIL.to_string() 
    };  

    // Wipe entire map instead of inserting new site to ensure data is synchronized 
    let mut writeable_sites = sites_data.write().await;
    writeable_sites.clear();
    writeable_sites.append(&mut recurse_sites.into_iter()
        .map(|site| (site.website_id, site))
        .collect::<BTreeMap<u32, SiteData>>());
    
    locked_gh_client.repos(GH_USER, GH_REPO)
        .update_file(
            GH_SITES_PATH,
            format!("Automated Action: Added a new site for '{}'", form.url),
            json_output,
            &sites_sha)
        .branch("main")
        .commiter(commit_author.clone())
        .author(commit_author)
        .send().await
        .context("Could not update GitHub site files")?;

    Ok(Redirect::to(format!("/?id={}&uuid_str={}", max_website_id + 1, uuid_str)))
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

// Backwards compatible static javascript file
#[get("/ring.js")]
async fn static_javascript() -> Option<NamedFile> {
    let path = Path::new(relative!("static")).join("ring.js");
    NamedFile::open(path).await.ok()
}

#[get("/sites.json")]
async fn dynamic_json(sites_data: &State<SitesMap>) -> Json<Vec<SiteData>> {
    let serializable_sites = sites_data.read().await
        .values().cloned()
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

    let ordered_sites = initial_site_data.into_iter()
        .map(|site| (site.website_id, site))
        .collect::<BTreeMap<u32, SiteData>>();
    
    rocket::build()
        .manage(Mutex::new(octocrab))
        .manage(Mutex::new(config))
        .manage(RwLock::new(ordered_sites) as SitesMap)
        .attach(oauth::recurse_oauth_fairing())
        .attach(Template::fairing())
        .configure(Config::figment().merge(("port", 4000)))
        .mount("/", routes![authed, home, add, logout, prev, next, random, 
            dynamic_json, static_javascript, health])
        .mount("/static/", FileServer::from(relative!("static")))
}
