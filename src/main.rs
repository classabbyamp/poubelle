use std::{collections::HashMap, path::{self, Path}};

use clap::Parser;
use mime2ext::mime2ext;
use rand::Rng;
use sqids;

use actix_files as fs;
use actix_multipart::form::{tempfile::{TempFile, TempFileConfig}, MultipartForm};
use actix_web::{get, post, web, App, Error, HttpRequest, HttpResponse, HttpServer, Responder};
use actix_web::http::header::{ContentDisposition, DispositionType};
use actix_web::middleware::Logger;

#[derive(Debug, MultipartForm)]
struct UploadForm {
    #[multipart(rename = "file")]
    files: Vec<TempFile>,
}

#[get("/")]
async fn index() -> impl Responder {
    HttpResponse::Ok().body("meow")
}

#[post("/")]
async fn upload(
    MultipartForm(form): MultipartForm<UploadForm>,
    sq: web::Data<sqids::Sqids>,
    cfg: web::Data<Config>,
) -> Result<impl Responder, Error> {
    let mut ids: Vec<String> = vec![];
    let mut rng = rand::thread_rng();
    let mut id;
    for f in form.files {
        let now = std::time::SystemTime::now();
        let unix = now.duration_since(std::time::UNIX_EPOCH).expect("ruh-roh").as_secs();
        let random: u64 = rng.gen_range(1..1000);
        id = sq.encode(&[unix, random]).unwrap();
        ids.push(id.clone());

        let filename: path::PathBuf = f.file_name.unwrap_or("file".into()).into();
        let mut ext = if let Some(mime) = f.content_type {
            if mime != mime::APPLICATION_OCTET_STREAM {
                mime2ext(mime)
            } else { None }
        } else { None };
        if ext.is_none() {
            ext = match filename.extension() {
                Some(e) => e.to_str(),
                None => match tree_magic_mini::from_file(f.file.as_file()) {
                    Some(m) => mime2ext(m),
                    None => None,
                }
            }
        }

        let mut path = cfg.upload_dir.clone();
        path.push(id);
        if let Some(e) = ext {
            path.set_extension(e);
        }
        f.file.persist(path).unwrap();
    }
    Ok(
        HttpResponse::Ok().body(
            ids.into_iter().map(|id| format!("{}/{}", cfg.baseurl, id)).collect::<Vec<String>>().join("\n") + "\n"
        )
    )
}

#[get("/{id}")]
async fn serve_id(req: HttpRequest, cfg: web::Data<Config>) -> Result<fs::NamedFile, Error> {
    let file_id: String = req.match_info().query("id").parse().unwrap();
    serve_file(file_id, None).await
}

#[get("/{id}/{fn}")]
async fn serve_name(req: HttpRequest, cfg: web::Data<Config>) -> Result<fs::NamedFile, Error> {
    let file_id: String = req.match_info().query("id").parse().unwrap();
    let filename: path::PathBuf = req.match_info().query("fn").parse().unwrap();
    let ext = match filename.extension() {
        Some(e) => e.to_str(),
        None => None,
    };
    serve_file(file_id, ext).await
}

async fn serve_file(
    id: String,
    ext: Option<&str>,
) -> Result<fs::NamedFile, Error> {
    let mut path = dir.clone();
    path.push(&id);

    let mut file = fs::NamedFile::open(path)?;

    if let Some(e) = ext {
        file = file.set_content_type(fs::file_extension_to_mime(e));
    }

    Ok(file
        .set_content_disposition(ContentDisposition {
            disposition: DispositionType::Inline,
            parameters: vec![],
        })
    )
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
    #[arg(short, long, env = "POUBELLE_UPLOAD_DIR", default_value = "./uploads")]
    /// directory to store uploads in
    upload_dir: path::PathBuf,
}

// struct Cache {
//     inner: HashMap<&str, &str>,
// }

// impl Cache {
//     fn init(dir: &Path) -> std::io::Result<Self> {
//         let mut inner = HashMap::new();
//         if dir.is_dir() {
//             for entry in std::fs::read_dir(dir)? {
//                 let entry = entry?;
//                 let path = entry.path();
//                 if path.is_file() {
//                     inner.insert(path.file_stem(), path.extension());
//                 }
//             }
//         }
//         Ok(Self { inner })
//     }
// }

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    let cfg = Config::parse();
    let config = web::Data::new(cfg.clone());

    // TODO: global hashmap of id -> filename
    // TODO: take all but first as extension

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(web::Data::clone(&config))
            .app_data(web::Data::new(sqids::Sqids::default()))
            //.app_data(web::Data::new())
            .app_data(TempFileConfig::default().directory(cfg.upload_dir.clone()))
            .service(index)
            .service(upload)
            .service(serve_id)
            .service(serve_name)
    })
    .bind((cfg.address, cfg.port))?
    .run()
    .await
}
