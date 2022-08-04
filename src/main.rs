use std::borrow::Cow;

use axum::body::{Bytes, Full, HttpBody, Body};
use axum::extract::Path;
use axum::http::{Response, HeaderMap};
use axum::response::{IntoResponse};
use axum::{
    routing::get,
    Router,
};
use lol_html::{rewrite_str, RewriteStrSettings, element, ElementContentHandlers, Selector, text};
use regex::{Regex, Captures};
use reqwest::{Url, Response as ReqwestResponse};
use lazy_static::lazy_static;

struct ProxyResult {
    content_type: Option<String>,
    headers: HeaderMap,
    response: ReqwestResponse
    // bytes: Bytes
}

async fn attempt_proxy(url: &str) -> Result<ProxyResult, reqwest::Error> {
    let response = reqwest::get(&url[1..]).await?;

    let mut simple_content_type = None;
    if let Some(ty) = response.headers().get("content-type") {
        if let Ok(s) = ty.to_str() {
            if let Some(split) = s.split_once(";") {
                simple_content_type = Some(split.0.to_owned());
            } else {
                simple_content_type = Some(s.to_owned());
            }
        }
    }

    let headers = response.headers().clone();
    // let bytes = response.bytes().await?;

    Ok(ProxyResult {
        content_type: simple_content_type,
        headers,
        response
        // bytes
    })
}

fn element_handler<'a>(tag: &'a str, attr: &'a str, urlobj: &'a Url) -> (Cow<'a, Selector>, ElementContentHandlers<'a>)
{
    let selector = format!("{}[{}]", tag, attr);

    element!(&selector, move |el| {
        let attr_value = el.get_attribute(attr)
            .expect("Always has attr");

        if attr_value.starts_with("data:") {
            return Ok(());
        }

        let new = match Url::parse(&attr_value) {
            Ok(url) => url,
            Err(_) => urlobj.join(&attr_value).expect("attr should be valid")
        };
        let abs_rel = format!("/{}", new.as_str());

        // dbg!(&attr_value, &abs_rel);

        el.set_attribute(&attr, &abs_rel).expect("new attr should be valid");

        Ok(())
    })
}

fn replace_css(root: Url, css_str: &str) -> String {
    lazy_static! {
        static ref URL_REGEX: Regex = Regex::new(r#"url\((?:"(.+?)"|'(.+?)'|(.+?))\)"#).unwrap();
    }

    let css_str = URL_REGEX.replace_all(&css_str, |caps: &Captures| {
        let inner = caps.get(1)
            .or_else(|| caps.get(2))
            .or_else(|| caps.get(3))
            .unwrap()
            .as_str();

        let new = root.join(inner);

        // dbg!(&root, &new);

        match new {
            Err(_) => caps[0].to_owned(),
            Ok(url) => format!("url({})", url)
        }
    }).to_string();

    return css_str;
}

async fn main_echo_prefix(
    Path(url): Path<String>,
    headers: HeaderMap
) -> impl IntoResponse {
    let mut response = Response::builder();

    println!("{}", url);

    if url == "/" || url.is_empty() {
        return response.body(Body::from("empty").boxed()).unwrap().into_response();
    } else {
        let urlobj = match Url::parse(&url[1..]) {
            Err(_) => return response.status(404).body(Body::from("")).unwrap().into_response(),
            Ok(url) => url
        };
        let proxied = match attempt_proxy(&url).await {
            Err(_) => return response.body(Body::from("error when proxying")).unwrap().into_response(),
            Ok(r) => r
        };

        match proxied.content_type.as_ref().map(|s| s.as_str()) {
            Some("text/html") => {
                for (name, value) in proxied.headers.iter() {
                    if name.as_str() != "content-length" {
                        response = response.header(name, value);
                    }
                }

                let bytes_str = String::from_utf8(proxied.response.bytes().await.unwrap().to_vec()).unwrap();

                let rewritten = rewrite_str(&bytes_str, RewriteStrSettings {
                    element_content_handlers: vec![
                        element_handler("script", "src", &urlobj),
                        element_handler("link", "href", &urlobj),
                        element_handler("img", "src", &urlobj),
                        element_handler("a", "href", &urlobj),
                        // text!("style", |el| {
                        //     let inner = el.as_str();

                        //     dbg!(inner);

                        //     Ok(())
                        // })
                    ],
                    ..Default::default()
                });

                response
                    // .header("content-type", "text/plain")
                    .body(Full::new(Bytes::from(rewritten.unwrap())))
                    .unwrap()
                    .into_response()
            },
            Some("text/css") => {
                for (name, value) in proxied.headers.iter() {
                    if name.as_str() != "content-length" {
                        response = response.header(name, value);
                    }
                }

                let root = headers.get(axum::http::header::REFERER);
                dbg!(&root);

                let root = match root {
                    Some(root) => Url::parse(root.to_str().unwrap()),
                    None => Ok(urlobj)
                };
                let root = root.unwrap();

                let css_str = replace_css(root, &String::from_utf8(proxied.response.bytes().await.unwrap().to_vec()).unwrap());

                response
                    // .header("content-type", "text/plain")
                    .body(Full::new(Bytes::from(css_str)))
                    .unwrap()
                    .into_response()
            },
            ct => {
                println!("defer {:?}", ct);

                for header in proxied.headers.iter() {
                    response = response.header(header.0, header.1);
                }

                response
                    // STREAMINGGGG
                    .body(Body::wrap_stream(proxied.response.bytes_stream()))
                    .unwrap()
                    .into_response()

            }
        }
    }
}

#[tokio::main]
async fn main() {
    // build our application with a single route
    let app = Router::new()
        // .route("/", get(|| async { "Hello, World!" }))
        .route("/*path", get(main_echo_prefix));

    // run it with hyper on localhost:3000
    let port = std::env::var("PORT").unwrap_or(String::from("3000"));
    let url = format!("0.0.0.0:{port}");
    axum::Server::bind(&url.parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}
