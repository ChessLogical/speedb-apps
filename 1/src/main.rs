use actix_files::NamedFile;
use actix_multipart::Multipart;
use actix_web::{error, web, App, Error, HttpResponse, HttpServer, Responder, Result};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sled::Db;
use std::io::Write;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct Post {
    title: String,
    message: String,
    file_path: Option<String>,
}

#[derive(Serialize)]
struct FormData {
    posts: Vec<Post>,
}

async fn index() -> Result<NamedFile> {
    Ok(NamedFile::open("static/index.html")?)
}

async fn submit_post(
    mut payload: Multipart,
    data: web::Data<AppState>,
) -> Result<HttpResponse, Error> {
    let mut title = String::new();
    let mut message = String::new();
    let mut file_path: Option<String> = None;

    while let Ok(Some(mut field)) = payload.try_next().await {
        let content_disposition = field.content_disposition().ok_or(error::ErrorBadRequest("Invalid content disposition"))?;
        let field_name = content_disposition.get_name().ok_or(error::ErrorBadRequest("Invalid field name"))?;

        match field_name {
            "title" => {
                while let Some(chunk) = field.try_next().await? {
                    title.push_str(std::str::from_utf8(&chunk).map_err(|_| error::ErrorBadRequest("Invalid UTF-8 in title"))?);
                }
            }
            "message" => {
                while let Some(chunk) = field.try_next().await? {
                    message.push_str(std::str::from_utf8(&chunk).map_err(|_| error::ErrorBadRequest("Invalid UTF-8 in message"))?);
                }
            }
            "file" => {
                let filename = format!("static/uploads/{}", Uuid::new_v4().to_string());
                let filepath = PathBuf::from(&filename);
                let mut f = web::block(|| std::fs::File::create(filepath))
                    .await
                    .map_err(|_| error::ErrorInternalServerError("File creation error"))?;

                while let Some(chunk) = field.try_next().await? {
                    f = web::block(move || {
                        f.write_all(&chunk)?;
                        Ok::<_, std::io::Error>(f)
                    })
                    .await
                    .map_err(|_| error::ErrorInternalServerError("File write error"))??;
                }
                file_path = Some(filename);
            }
            _ => (),
        }
    }

    if title.is_empty() || message.is_empty() {
        return Err(error::ErrorBadRequest("Title and message are required"));
    }

    if title.len() > 15 || message.len() > 200000 {
        return Err(error::ErrorBadRequest("Title or message exceeds the limit"));
    }

    let post = Post {
        title,
        message,
        file_path,
    };

    data.db
        .insert(Uuid::new_v4().to_string(), serde_json::to_vec(&post).map_err(|_| error::ErrorInternalServerError("Serialization error"))?)
        .map_err(|_| error::ErrorInternalServerError("Database insert error"))?;
    data.db.flush().map_err(|_| error::ErrorInternalServerError("Database flush error"))?;

    Ok(HttpResponse::SeeOther()
        .append_header(("Location", "/"))
        .finish())
}

async fn get_posts(data: web::Data<AppState>) -> impl Responder {
    let mut posts = Vec::new();

    for item in data.db.iter() {
        if let Ok((_, value)) = item {
            if let Ok(post) = serde_json::from_slice(&value) {
                posts.push(post);
            }
        }
    }

    HttpResponse::Ok().json(FormData { posts })
}

struct AppState {
    db: Db,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let db = sled::open("posts_db").unwrap();
    let app_data = web::Data::new(AppState { db });

    HttpServer::new(move || {
        App::new()
            .app_data(app_data.clone())
            .service(actix_files::Files::new("/static", "static"))
            .route("/", web::get().to(index))
            .route("/submit", web::post().to(submit_post))
            .route("/posts", web::get().to(get_posts))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
