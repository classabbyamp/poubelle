use std::collections::HashMap;
use std::path;

use clap::Parser;
use log::{info, debug};
use mime::Mime;
use parking_lot::RwLock;
use rand::Rng;
use sqids;
use tokio::process::Command;

use actix_files as fs;
use actix_multipart::form::{tempfile::{TempFile, TempFileConfig}, MultipartForm};
use actix_web::{get, post, web, App, Error, HttpRequest, HttpResponse, HttpServer, Responder};
use actix_web::http::header::{ContentDisposition, DispositionType};
use actix_web::middleware::Logger;

const CLI_UA: [&str; 5] = ["wget", "curl", "fetch", "powershell", "ansible-httpget"];

#[derive(Debug, MultipartForm)]
struct UploadForm {
    #[multipart(rename = "file")]
    files: Vec<TempFile>,
}

#[get("/")]
async fn index() -> impl Responder {
    // TODO: an actual homepage
    HttpResponse::Ok().body("meow")
}

#[post("/")]
async fn upload(
    MultipartForm(form): MultipartForm<UploadForm>,
    sq: web::Data<sqids::Sqids>,
    cfg: web::Data<Config>,
    cache: web::Data<Cache>,
) -> Result<impl Responder, Error> {
    // TODO: secure this
    let mut ids: Vec<String> = vec![];
    let mut rng = rand::thread_rng();
    let mut id;

    for f in form.files {
        // TODO: strip exif?
        // generate an id for the file based on the time and a random number
        // at the scale this will operate at, it is unlikely to have collisions
        let now = std::time::SystemTime::now();
        let unix = now.duration_since(std::time::UNIX_EPOCH).expect("ruh-roh").as_secs();
        let random: u64 = rng.gen_range(1..1000);
        id = sq.encode(&[unix, random]).unwrap();
        ids.push(id.clone());

        let mime = get_mime(f.file.path()).await?;

        let mut cache = cache.write();
        cache.insert(id.clone(), mime);

        let mut p = cfg.upload_dir.clone();
        p.push(id);
        f.file.persist(p).unwrap();
    }

    // respond with a list of urls for the uploaded files, in the same order
    Ok(
        HttpResponse::Ok().body(
            ids.into_iter()
                .map(|id| format!("{}/{}", cfg.baseurl, id))
                .collect::<Vec<String>>().join("\n")
            + "\n"
        )
    )
}

#[get("/{id}{rest:.*}")]
async fn serve_rich(
    p: web::Path<(String, String)>,
    req: HttpRequest,
    cfg: web::Data<Config>,
    cache: web::Data<Cache>
) -> Result<fs::NamedFile, Error> {
    let (file_id, _rest) = p.into_inner();

    let cache = cache.read();
    let mime = cache.get(&file_id);
    let mut pretty = false;

    if mime.type_() == mime::TEXT {
        if let Some(ua) = req.headers().get("User-Agent") {
            if ! CLI_UA.iter().any(|x| ua.to_str().unwrap_or_default().contains(x)) {
                pretty = true;
            }
        }
    }

    render_file(format!("{}/{}", cfg.upload_dir.to_string_lossy(), file_id), &mime, pretty).await
}

#[get("/r/{id}{rest:.*}")]
async fn serve_raw(
    p: web::Path<(String, String)>,
    cfg: web::Data<Config>,
    cache: web::Data<Cache>
) -> Result<fs::NamedFile, Error> {
    let (file_id, _rest) = p.into_inner();

    let cache = cache.read();
    let mime = cache.get(&file_id);

    render_file(format!("{}/{}", cfg.upload_dir.to_string_lossy(), file_id), &mime, false).await
}

async fn render_file(
    path: String,
    mime: &Mime,
    pretty: bool,
) -> Result<fs::NamedFile, Error> {
    info!("rendering file {path} as {mime}{}", if pretty { " all pretty-like" } else { "" });

    let f = fs::NamedFile::open(path)?;

    Ok(f.set_content_type(mime.clone())
        .set_content_disposition(
            ContentDisposition {
                disposition: DispositionType::Inline,
                parameters: vec![],
            }
        )
    )
}

async fn get_mime(path: impl AsRef<std::ffi::OsStr>) -> std::io::Result<Mime> {
    let cmd = Command::new("file")
        .arg("-b")
        .arg("-E")
        .arg("--mime-type")
        .arg(path)
        .output();
    let output = cmd.await?;
    if output.status.success() {
        match std::str::from_utf8(output.stdout.as_slice()) {
            Ok(s) => Ok(s.parse().unwrap_or(mime::TEXT_PLAIN)),
            Err(_) => Err(std::io::Error::from_raw_os_error(95))
        }
    } else {
        Err(std::io::Error::from_raw_os_error(output.status.code().unwrap_or_default()))
    }
}

type Cache = RwLock<MimeCache>;

struct MimeCache {
    _inner: HashMap<String, Mime>,
}

impl MimeCache {
    async fn init(dir: path::PathBuf) -> std::io::Result<Self> {
        let mut inner = HashMap::new();

        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Ok(m) = get_mime(&path).await {
                    if let Some(k) = path.file_name() {
                        let k = k.to_string_lossy().into();
                        debug!("cache init: {} -> {}", k, m);
                        inner.insert(k, m);
                    }
                }
            }
        }

        Ok(Self { _inner: inner })
    }

    fn insert(&mut self, file_id: String, mime: Mime) -> () {
        debug!("cache insert: {file_id} -> {mime}");
        self._inner.insert(file_id, mime);
    }

    // fn remove(&mut self, file_id: &String) -> () {
    //     debug!("cache remove: {file_id}");
    //     self._inner.remove(file_id);
    // }

    fn get(&self, file_id: &String) -> &Mime {
        debug!("cache get: {file_id}");
        self._inner.get(file_id).unwrap_or(&mime::TEXT_PLAIN)
    }
}

#[derive(Parser, Clone)]
#[command(version, about, long_about = None)]
struct Config {
    #[arg(short, long, env = "POUBELLE_BASEURL", default_value = "http://localhost:8080")]
    /// address to display to users
    baseurl: String,
    #[arg(short, long, env = "POUBELLE_ADDRESS", default_value = "localhost")]
    /// address to bind to
    address: String,
    #[arg(short, long, env = "POUBELLE_PORT", default_value = "8080")]
    /// port to bind to
    port: u16,
    #[arg(short, long, env = "POUBELLE_UPLOAD_DIR", default_value = "./uploads", value_parser = check_upload_dir)]
    /// directory to store uploads in
    upload_dir: path::PathBuf,
}

fn check_upload_dir(s: &str) -> Result<path::PathBuf, String> {
    let p: path::PathBuf = s.parse().unwrap();

    if p.exists() {
        if p.is_dir() {
            Ok(p)
        } else {
            Err(format!("'{s}' is not a directory"))
        }
    } else {
        info!("creating upload directory '{s}'");
        match std::fs::create_dir_all(&p) {
            Ok(_) => Ok(p),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    let cfg = Config::parse();
    let config = web::Data::new(cfg.clone());
    let cache = web::Data::new(
        RwLock::new(
            MimeCache::init(cfg.upload_dir.clone()).await?
        )
    );

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(web::Data::clone(&config))
            .app_data(web::Data::new(sqids::Sqids::default()))
            .app_data(web::Data::clone(&cache))
            .app_data(TempFileConfig::default().directory(cfg.upload_dir.clone()))
            .service(index)
            .service(upload)
            .service(serve_raw)
            .service(serve_rich)
    })
    .bind((cfg.address, cfg.port))?
    .run()
    .await
}
