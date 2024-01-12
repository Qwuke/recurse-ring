use rocket::{
    http::{Cookie, CookieJar, Status, uri::{Uri, Absolute}},
    form::Form,
    request::{FromRequest, Outcome},
    response::{Redirect, Debug},
    fs::{FileServer, relative},
    serde::{Serialize, Deserialize, json::Json}, 
    State, request, form, Config};
use tokio::sync::Mutex;
use anyhow::{Error, Result, anyhow, Context};
use reqwest::Client;
use rocket_dyn_templates::{Template, context};
use figment::{Figment, providers::Env};
use octocrab::{Octocrab, models::repos::{CommitAuthor, Content}};
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

#[derive(Serialize, Deserialize)]
struct SiteData {
    website_id: u32,
    website_uuid: String,
    recurse_id: u32,
    website_name: String,
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

async fn get_site_content(octocrab: &Mutex<Octocrab>) -> Result<Content> {
    let all_contents = octocrab.lock().await.repos(GH_USER, GH_REPO)
        .get_content()
        .path(GH_SITES_PATH)
        .r#ref("main")
        .send().await?;

    all_contents.items.into_iter()
        .next()
        .ok_or(anyhow!("Missing GitHub content for {}", GH_SITES_PATH))
}

async fn get_deserialized_sites(octocrab: &Mutex<Octocrab>) -> Result<Vec<SiteData>> {
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
async fn authed(user: User, gh_client: &State<Mutex<Octocrab>>, id: Option<u32>, uuid_str: Option<String>) -> Result<Template, Debug<Error>> {
    let recurse_sites: Vec<SiteData> = get_deserialized_sites(gh_client).await?;
    let sites_with_names = get_named_sites(recurse_sites, user.token.as_ref().unwrap()).await?;

    Ok(Template::render("index", context! { sites: sites_with_names, user, id, uuid_str }))
}

#[get("/", rank = 2)]
async fn home(gh_client: &State<Mutex<Octocrab>>, client_tokens: &State<Mutex<ClientTokens>>) -> Result<Template, Debug<Error>> {
    let recurse_sites: Vec<SiteData> = get_deserialized_sites(gh_client).await?;
    let sites_with_names = get_named_sites(recurse_sites, &client_tokens.lock().await.recurse_secret).await?;

    Ok(Template::render("index", context! { sites: sites_with_names }))
}

#[post("/auth/add", data = "<form>")]
async fn add(user: User, form: Form<WebsiteSignup>, gh_client: &State<Mutex<Octocrab>>) -> Result<Redirect, Debug<Error>> {   
    let sites_sha = get_site_content(gh_client).await?.sha;
    let mut recurse_sites: Vec<SiteData> = get_deserialized_sites(gh_client).await?;
    let max_website_id = recurse_sites.iter()
        .max_by_key(|site| site.website_id)
        .ok_or(anyhow!("No maximum site found"))?
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
    let output = serde_json::to_string_pretty::<Vec<SiteData>>(&recurse_sites)
        .context("Could not prettify serialized sites")?;

    let commit_author = CommitAuthor {
        name: GH_COMMITTER_NAME.to_string(),
        email: GH_COMMITTER_EMAIL.to_string() 
    };

    gh_client.lock().await.repos(GH_USER, GH_REPO)
        .update_file(
            GH_SITES_PATH,
            format!("Automated Action: Added a new site for '{}'", form.url),
            output,
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

#[get("/prev?<id>")]
async fn prev(id: u32, gh_client: &State<Mutex<Octocrab>>) -> Result<Redirect, (Status, Debug<Error>)> {
    let mut recurse_sites: Vec<SiteData> = get_deserialized_sites(gh_client).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;
    
    recurse_sites.sort_by_key(|site| site.website_id);
    
    let site_position = recurse_sites.iter().position(|site| site.website_id == id)
        .ok_or((Status::NotFound, Debug(anyhow!("Website ID not found"))))?;

    let redirect_site 
        = if site_position < 1 {
            recurse_sites.last().unwrap()
        } else { 
            recurse_sites.get(site_position - 1).unwrap() 
        };

    Ok(Redirect::to(redirect_site.url.clone()))
}

#[get("/next?<id>")]
async fn next(id: u32, gh_client: &State<Mutex<Octocrab>>) -> Result<Redirect, (Status, Debug<Error>)> {
    let mut recurse_sites: Vec<SiteData> = get_deserialized_sites(gh_client).await
        .map_err(|e| (Status::InternalServerError, Debug(e)))?;
    
    recurse_sites.sort_by_key(|site| site.website_id);

    let site_position = recurse_sites.iter().position(|site| site.website_id == id)
        .ok_or((Status::NotFound, Debug(anyhow!("Website ID not found"))))?;

    let redirect_site 
        = if site_position + 1 == recurse_sites.len()  {
            recurse_sites.first().unwrap()
        } else {
            recurse_sites.get(site_position + 1).unwrap()
        };

    Ok(Redirect::to(redirect_site.url.clone()))
}

#[get("/rand")]
async fn random(gh_client: &State<Mutex<Octocrab>>) -> Result<Redirect, Debug<Error>> {
    let site_data: Vec<SiteData> = get_deserialized_sites(gh_client).await?;
    let mut rng = thread_rng();
    let non_main_site_index = rng.gen_range(1..site_data.len());
    let random_site = site_data.into_iter().nth(non_main_site_index).unwrap();
    Ok(Redirect::to(random_site.url))
}

#[get("/static/sites.json")]
async fn static_sites(gh_client: &State<Mutex<Octocrab>>) -> Json<Vec<SiteData>> {
    let site_data = get_deserialized_sites(gh_client).await.unwrap();
    Json(site_data)
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

    rocket::build()
        .manage(Mutex::new(octocrab))
        .manage(Mutex::new(config))
        .attach(oauth::recurse_oauth_fairing())
        .attach(Template::fairing())
        .configure(Config::figment().merge(("port", 4000)))
        .mount("/", routes![authed, home, add, logout, prev, next, random, static_sites, health])
        .mount("/static/", FileServer::from(relative!("static")))
}
