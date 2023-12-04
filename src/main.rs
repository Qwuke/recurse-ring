use tokio::sync::Mutex;
use serde::{Serialize, Deserialize};
use rocket::{
    http::{Cookie, CookieJar, Status, 
        uri::{Uri, Absolute}},
    form::Form,
    request::{FromRequest, Outcome},
    response::{Redirect, status::{NotFound, Accepted}},
    fs::{FileServer, relative},
    State, request, form, Config};
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
const RECURSE_OAUTH_URL: &str = "https://www.recurse.com/oauth/authorize";
const RECURSE_TOKEN_URL: &str = "https://www.recurse.com/oauth/token";

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

fn valid_domain<'v>(input_url: &String) -> form::Result<'v, ()> {
    let maybe_uri = Uri::parse::<Absolute>(input_url.as_str());
    let parsed_uri = maybe_uri.unwrap();
    let host_domain = parsed_uri
        .absolute().unwrap()
        .authority().unwrap()
        .host();
    let domain = parse_domain_name(host_domain)
        .or_else(|e| { println!("{}", e); Err(form::Error::validation("Please use a parseable domain name")) })? ;
    
    if !domain.has_known_suffix() {
        Err(form::Error::validation("Please use a valid domain name"))?;
    }

    Ok(())
}

async fn get_site_content(octocrab: &Mutex<Octocrab>) -> Content {
    let all_contents = octocrab.lock().await.repos(GH_USER, GH_REPO)
    .get_content()
    .path(GH_SITES_PATH)
    .r#ref("main")
    .send()
    .await.unwrap();

    all_contents.items.first().unwrap().clone()
}

async fn get_named_sites(unnamed_sites: Vec<SiteData>, bearer_token: &String) -> Vec<SiteData> {
    let mut sites_with_names = Vec::new();

    for site in unnamed_sites {
        if site.website_id != 0 {
            let response = Client::new()
                .get(format!("{}profiles/{}", RECURSE_BASE_URL, site.recurse_id))
                .bearer_auth(bearer_token)
                .send().await.unwrap();
            let res = response.text().await.unwrap();
            let Ok(user) = serde_json::from_str::<User>(res.as_str()) else {
                return Vec::new();
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

    return sites_with_names;
}

#[get("/?<id>&<uuid_str>")]
async fn authed(user: User, gh_client: &State<Mutex<Octocrab>>, id: Option<u32>, uuid_str: Option<String>) -> Template {
    let encoded_sites = get_site_content(gh_client.inner()).await;
    let site_data: Vec<SiteData> = serde_json::from_str(encoded_sites.decoded_content().as_ref().unwrap()).unwrap();
    let sites_with_names = get_named_sites(site_data, user.token.as_ref().unwrap()).await;

    Template::render("index", context! { sites: sites_with_names, user, id, uuid_str })
}

#[get("/", rank = 2)]
async fn home(gh_client: &State<Mutex<Octocrab>>, client_tokens: &State<Mutex<ClientTokens>>) -> Template {
    let encoded_sites = get_site_content(gh_client.inner()).await;
    let site_data: Vec<SiteData> = serde_json::from_str(encoded_sites.decoded_content().as_ref().unwrap()).unwrap();

    let sites_with_names = get_named_sites(site_data, &client_tokens.lock().await.recurse_secret).await;
    Template::render("index", context! { sites: sites_with_names })
}

#[post("/auth/add", data = "<form>")]
async fn add(user: User, form: Form<WebsiteSignup>, gh_client: &State<Mutex<Octocrab>>) -> Redirect {    
    let encoded_sites = get_site_content(gh_client.inner()).await;
    let mut site_data: Vec<SiteData> = serde_json::from_str(encoded_sites.decoded_content().as_ref().unwrap()).unwrap();
    let max_website_id = site_data.iter().max_by_key(|site| site.website_id).unwrap().website_id;
    let uuid_str = Uuid::new_v4().to_string();
    let new_site = SiteData {
        website_id: max_website_id + 1,
        website_uuid: uuid_str.clone(),
        recurse_id: user.id, 
        website_name: form.name.clone(), 
        recurse_name: None,
        url: form.url.clone(), 
    };

    site_data.push(new_site);

    let output = serde_json::to_string_pretty::<Vec<SiteData>>(&site_data).unwrap();

    let commit_author = CommitAuthor {
        name: GH_COMMITTER_NAME.to_string(),
        email: GH_COMMITTER_EMAIL.to_string() 
    };

    gh_client.lock().await.repos(GH_USER, GH_REPO)
        .update_file(
            GH_SITES_PATH,
            format!("Automated Action: Added a new site for '{}'", form.url),
            output,
            &encoded_sites.sha)
        .branch("main")
        .commiter(commit_author.clone())
        .author(commit_author)
        .send()
        .await.unwrap();

    Redirect::to(format!("/?id={}&uuid_str={}", max_website_id + 1, uuid_str))
}

#[get("/auth/logout")]
fn logout(_user: User, cookies: &CookieJar<'_>) -> Redirect {
    cookies.remove(Cookie::from("name"));
    cookies.remove(Cookie::from("id"));
    cookies.remove(Cookie::from("api_token"));
    Redirect::to("/")
}

#[get("/prev?<id>")]
async fn prev(id: u32, gh_client: &State<Mutex<Octocrab>>) -> Result<Redirect, NotFound<String>> {
    let encoded_sites = get_site_content(gh_client.inner()).await;
    let mut site_data: Vec<SiteData> = serde_json::from_str(encoded_sites.decoded_content().as_ref().unwrap()).unwrap();
    site_data.sort_by_key(|site| site.website_id);
    let site_position = site_data.iter().position(|site| site.website_id == id).ok_or(NotFound("Website ID not found".to_owned()))?;

    let redirect_site = match site_position < 1 {
        true => site_data.last().unwrap(),
        false => site_data.get(site_position - 1).unwrap(),
    };

    Ok(Redirect::to(redirect_site.url.clone()))
}

#[get("/next?<id>")]
async fn next(id: u32, gh_client: &State<Mutex<Octocrab>>) -> Result<Redirect, NotFound<String>> {
    let encoded_sites = get_site_content(gh_client.inner()).await;
    let mut site_data: Vec<SiteData> = serde_json::from_str(encoded_sites.decoded_content().as_ref().unwrap()).unwrap();
    site_data.sort_by_key(|site| site.website_id);
    let site_position = site_data.iter().position(|site| site.website_id == id).ok_or(NotFound("Website ID not found".to_owned()))?;

    let redirect_site = match site_position + 1 == site_data.len()  {
        true => site_data.first().unwrap(),
        false => site_data.get(site_position + 1).unwrap(),
    };

    Ok(Redirect::to(redirect_site.url.clone()))
}

#[get("/rand")]
async fn random(gh_client: &State<Mutex<Octocrab>>) -> Redirect {
    let encoded_sites = get_site_content(gh_client.inner()).await;
    let site_data: Vec<SiteData> = serde_json::from_str(encoded_sites.decoded_content().as_ref().unwrap()).unwrap();
    let mut rng = thread_rng();
    let random_site = &site_data[rng.gen_range(1..site_data.len())];
    Redirect::to(random_site.url.clone())
}

#[get("/health")]
fn health() -> Accepted<String> { Accepted("pong!".to_owned()) }

#[launch]
#[tokio::main]
async fn rocket() -> _ {
    let config: ClientTokens = Figment::from(Env::prefixed("CONFIG_")).extract().expect("Config should be extractable");
    let octocrab = Octocrab::builder().personal_token(config.github_secret.clone()).build().unwrap();

    rocket::build()
        .manage(Mutex::new(octocrab))
        .manage(Mutex::new(config))
        .attach(oauth::recurse_oauth_fairing())
        .attach(Template::fairing())
        .configure(Config::figment().merge(("port", 4000)))
        .mount("/", routes![authed, home, add, logout, prev, next, random, health])
        .mount("/", FileServer::from(relative!("static")))
}
