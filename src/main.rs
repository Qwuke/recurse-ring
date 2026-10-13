#![feature(btree_cursors)]
use anyhow::{anyhow, Context, Result};
use figment::{providers::Env, Figment};
use octocrab::{models::repos::Content, Octocrab};
use rand::{thread_rng, Rng, prelude::SliceRandom};
use reqwest::Client;
use rocket::{
    fs::{relative, FileServer},
    http::{Cookie, CookieJar, Status, Method},
    request,
    request::{FromRequest, Outcome},
    response::Redirect,
    serde::{json::Json, Deserialize, Serialize},
    Config, State,
};
use rocket_cors::{AllowedOrigins, CorsOptions};
use rocket_dyn_templates::{context, Template};
use std::collections::BTreeMap;
use std::ops::Bound;
use tokio::sync::{Mutex, RwLock};

mod oauth;
mod sites;

#[macro_use]
extern crate rocket;

const GH_USER: &str = "Qwuke";
const GH_REPO: &str = "recurse-ring";
const GH_SITES_PATH: &str = "sites.json";

const RECURSE_BASE_URL: &str = "https://www.recurse.com/api/v1/";
const _RECURSE_OAUTH_URL: &str = "https://www.recurse.com/oauth/authorize";
const _RECURSE_TOKEN_URL: &str = "https://www.recurse.com/oauth/token";

#[derive(Deserialize)]
struct ClientTokens {
    _recurse_client_id: String,
    recurse_secret: String,
    github_secret: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct SiteData {
    website_id: u32,
    website_uuid: String,
    recurse_id: u32,
    website_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    recurse_name: Option<String>,
    #[serde(default)]
    is_anonymous: bool,
    url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
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
            .guard::<&CookieJar<'_>>().await
            .expect("Cookies should be accessible from the request");

        match (
            cookies.get_private("name"),
            cookies.get_private("id"),
            cookies.get_private("api_token"),
        ) {
            (Some(name), 
            Some(id), 
            Some(token)) => 
                Outcome::Success(User {
                    id: id.value().parse::<u32>().unwrap(),
                    name: name.value().to_string(),
                    token: Some(token.value().to_string()),
                }),
            _ => Outcome::Forward(Status::Unauthorized),
        }
    }
}

async fn get_site_content(octocrab: &Octocrab) -> Result<Content> {
    let all_contents = octocrab
        .repos(GH_USER, GH_REPO)
        .get_content()
        .path(GH_SITES_PATH)
        .r#ref("main")
        .send().await?;

    all_contents
        .items
        .into_iter()
        .next()
        .ok_or(anyhow!("Missing GitHub content for {}", GH_SITES_PATH))
}

async fn get_deserialized_sites(octocrab: &Octocrab) -> Result<Vec<SiteData>> {
    let decoded_sites = get_site_content(octocrab).await?
        .decoded_content()
        .ok_or(anyhow!("Could not decode GitHub content"))?;

    serde_json::from_str(&decoded_sites).context("Unable to deserialize sites from GitHub file")
}

async fn get_named_sites(
    unnamed_sites: Vec<SiteData>,
    bearer_token: &String,
) -> Result<Vec<SiteData>> {
    let mut sites_with_names = Vec::new();

    for site in unnamed_sites {
        let response = Client::new()
            .get(format!("{}profiles/{}", RECURSE_BASE_URL, site.recurse_id))
            .bearer_auth(bearer_token)
            .send().await?;
        let res = response.text().await?;
    
        let user_name = serde_json::from_str::<User>(res.as_str())
            .map(|user| user.name)
            .unwrap_or("Not A Real Recurser".to_string());
        
        sites_with_names.push(SiteData {
            website_id: site.website_id,
            website_uuid: site.website_uuid,
            recurse_id: site.recurse_id,
            website_name: site.website_name,
            recurse_name: Some(user_name),
            is_anonymous: site.is_anonymous,
            url: site.url,
        });
    }

    Ok(sites_with_names)
}

#[get("/?<id>&<uuid_str>")]
async fn authed(
    user: User,
    sites_data: &State<SitesMap>,
    id: Option<u32>,
    uuid_str: Option<String>,
) -> Template {
    let mut recurse_sites = sites_data
        .read().await
        .values()
        .cloned()
        .collect::<Vec<SiteData>>();

    let mut rng = thread_rng();
    recurse_sites.shuffle(&mut rng);
    
    Template::render(
        "index",
        context! { sites: recurse_sites, user, id, uuid_str })
}

#[get("/", rank = 2)]
async fn home(sites_data: &State<SitesMap>) -> Template {
    let mut recurse_sites = sites_data
        .read().await
        .values()
        .cloned()
        .collect::<Vec<SiteData>>();

    let mut rng = thread_rng();
    recurse_sites.shuffle(&mut rng);

    Template::render("index", context! { sites: recurse_sites })
}

#[get("/auth/logout")]
fn logout(_user: User, cookies: &CookieJar<'_>) -> Redirect {
    cookies.remove(Cookie::from("name"));
    cookies.remove(Cookie::from("id"));
    cookies.remove(Cookie::from("api_token"));
    Redirect::to("/")
}

#[get("/prev?<id>")]
async fn prev(id: u32, sites_data: &State<SitesMap>) -> Redirect {
    let readable_sites = sites_data.read().await;
    let last_key_value = readable_sites
        .last_key_value()
        .expect("Should always have the last site to wrap around to");
    let (_actual_id, prev_site) = readable_sites
        .upper_bound(Bound::Excluded(&id))
        .key_value()
        .unwrap_or(last_key_value);

    Redirect::to(prev_site.url.clone())
}

#[get("/next?<id>")]
async fn next(id: u32, sites_data: &State<SitesMap>) -> Redirect {
    let readable_sites = sites_data.read().await;
    let first_key_value = readable_sites
        .first_key_value()
        .expect("Should always have the first site to wrap around to");
    let (_actual_id, next_site) = readable_sites
        .lower_bound(Bound::Excluded(&id))
        .key_value()
        .unwrap_or(first_key_value);

    Redirect::to(next_site.url.clone())
}

#[get("/rand")]
async fn random(sites_data: &State<SitesMap>) -> Redirect {
    let readable_sites = sites_data.read().await;
    let mut rng = thread_rng();
    let non_main_site_index = rng.gen_range(1..readable_sites.len());
    let random_site = readable_sites
        .values()
        .nth(non_main_site_index)
        .expect("Should always have a random indice based on length");
    Redirect::to(random_site.url.clone())
}

#[get("/sites.json")]
async fn dynamic_json(sites_data: &State<SitesMap>) -> Json<Vec<SiteData>> {
    let serializable_sites = sites_data
        .read().await
        .values()
        .cloned()
        .map(|mut site| {
            site.recurse_name = None;
            site })
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

    let ordered_sites = sites_with_names
        .into_iter()
        .map(|site| (site.website_id, site))
        .collect::<BTreeMap<u32, SiteData>>();

    let cors = CorsOptions::default()
        .allowed_origins(AllowedOrigins::all())
        .allowed_methods(
            vec![Method::Get, Method::Post, Method::Patch]
                .into_iter()
                .map(From::from)
                .collect())
        .allow_credentials(true)
        .to_cors()
        .expect("CORS options should be valid");

    rocket::build()
        .manage(Mutex::new(octocrab))
        .manage(Mutex::new(config))
        .manage(RwLock::new(ordered_sites) as SitesMap)
        .attach(cors)
        .attach(oauth::recurse_oauth_fairing())
        .attach(Template::fairing())
        .configure(Config::figment().merge(("port", 4000)))
        .mount("/", routes![authed, home, sites::add, sites::update, 
            sites::delete, logout, prev, next, random, dynamic_json, health])
        .mount("/", FileServer::from(relative!("static")))
        .mount("/static/", FileServer::from(relative!("static")).rank(3))
}
