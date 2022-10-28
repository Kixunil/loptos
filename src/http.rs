use bip78::receiver::*;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};

use crate::scheduler::{self, ScheduledPayJoin, Scheduler};
use qrcode_generator::QrCodeEcc;
use std::net::SocketAddr;

#[cfg(not(feature = "test_paths"))]
const STATIC_DIR: &str = "/usr/share/loin/static";

#[cfg(feature = "test_paths")]
const STATIC_DIR: &str = "static";

fn create_qr_code(qr_string: &str, name: &str) {
    let filename = format!("{}/qr_codes/{}.png", STATIC_DIR, name);
    qrcode_generator::to_png_to_file(qr_string, QrCodeEcc::Low, 512, filename.clone())
        .expect(&format!("Saved QR code: {}", filename));
}

/// Serve requests to Schedule and execute PayJoins with given options.
pub async fn serve(
    sched: Scheduler,
    bind_addr: SocketAddr,
    endpoint: url::Url,
) -> Result<(), hyper::Error> {
    let new_service = make_service_fn(move |_| {
        let sched = sched.clone();
        let endpoint = endpoint.clone();
        async move {
            let handler = move |req| handle_web_req(sched.clone(), req, endpoint.clone());
            Ok::<_, hyper::Error>(service_fn(handler))
        }
    });

    let server = Server::bind(&bind_addr).serve(new_service);
    println!("Listening on: http://{}", bind_addr);
    server.await
}

async fn handle_web_req(
    scheduler: Scheduler,
    req: Request<Body>,
    endpoint: url::Url,
) -> Result<Response<Body>, hyper::Error> {
    use std::path::Path;

    match (req.method(), req.uri().path()) {
        (&Method::GET, "/pj") => {
            let index =
                std::fs::read(Path::new(STATIC_DIR).join("index.html")).expect("can't open index");
            Ok(Response::new(Body::from(index)))
        }

        (&Method::GET, path) if path.starts_with("/pj/static/") => {
            let directory_traversal_vulnerable_path = &path[("/pj/static/".len())..];
            let file =
                std::fs::read(Path::new(STATIC_DIR).join(directory_traversal_vulnerable_path))
                    .expect("can't open static file");
            Ok(Response::new(Body::from(file)))
        }

        (&Method::POST, "/pj") => {
            dbg!(req.uri().query());

            let headers = Headers(req.headers().to_owned());
            let query = {
                let uri = req.uri();
                if let Some(query) = uri.query() {
                    Some(&query.to_owned());
                }
                None
            };
            let body = req.into_body();
            let bytes = hyper::body::to_bytes(body).await?;
            dbg!(&bytes); // this is correct by my accounts
            let reader = &*bytes;
            let original_request = UncheckedProposal::from_request(reader, query, headers).unwrap();

            let proposal_psbt = scheduler.propose_payjoin(original_request).await.unwrap();

            Ok(Response::new(Body::from(proposal_psbt)))
        }

        (&Method::POST, "/pj/schedule") => {
            let bytes = hyper::body::to_bytes(req.into_body()).await?;
            // deserialize x-www-form-urlencoded data with non-strict encoded "channel[arrayindex]"
            let conf = serde_qs::Config::new(2, false);
            let request: ScheduledPayJoin =
                conf.deserialize_bytes(&bytes).expect("invalid request");

            let address = scheduler.schedule_payjoin(&request).await.unwrap();
            let total_amount = request.total_amount();
            let uri = scheduler::format_bip21(address.clone(), total_amount, endpoint);
            let mut response = Response::new(Body::from(uri.clone()));
            create_qr_code(&uri, &address.to_string());
            response
                .headers_mut()
                .insert(hyper::header::CONTENT_TYPE, "text/plain".parse().unwrap());
            Ok(response)
        }

        // Return the 404 Not Found for other routes.
        _ => {
            let mut not_found = Response::default();
            *not_found.status_mut() = StatusCode::NOT_FOUND;
            Ok(not_found)
        }
    }
}

pub(crate) struct Headers(hyper::HeaderMap);
impl bip78::receiver::Headers for Headers {
    fn get_header(&self, key: &str) -> Option<&str> {
        self.0.get(key)?.to_str().ok()
    }
}
