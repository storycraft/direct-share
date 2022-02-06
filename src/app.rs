/*
 * Created on Sat Feb 05 2022
 *
 * Copyright (c) storycraft. Licensed under the MIT Licence.
 */

use std::{
    error::Error, fmt::Display, net::SocketAddr, num::NonZeroU8, ops::RangeBounds, path::Path,
    sync::Arc,
};

use hyper::{
    header,
    server::conn::AddrStream,
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server, StatusCode,
};
use tokio::fs::File;
use tokio_util::codec::{BytesCodec, FramedRead};

use crate::{constants, map::PathMap};

#[derive(Debug)]
pub struct DirectShare {
    path_map: PathMap,
}

impl DirectShare {
    pub fn new(key_length: NonZeroU8) -> Self {
        Self {
            path_map: PathMap::new(key_length),
        }
    }

    /// Register path and return shorten key
    #[inline]
    pub fn register(&mut self, path: String) -> String {
        self.path_map.register(path)
    }

    async fn response(
        self: Arc<Self>,
        remote_addr: SocketAddr,
        req: Request<Body>,
    ) -> hyper::Result<Response<Body>> {
        if req.method() != Method::GET {
            return Ok(self.not_found_page());
        }

        let path = &req.uri().path()[1..];

        log::info!("Received path: {} from {}", path, remote_addr);

        Ok(self.deliver_file(path, ..).await)
    }

    async fn deliver_file<R: RangeBounds<u64>>(&self, path: &str, _: R) -> Response<Body> {
        let file_path = match self.path_map.get(path) {
            Some(file_path) => file_path,
            None => return self.not_found_page(),
        };

        match File::open(file_path).await {
            Ok(file) => {
                let stream = FramedRead::new(file, BytesCodec::new());
                let mut res = Response::new(Body::wrap_stream(stream));

                let name = Path::new(file_path)
                    .file_name()
                    .map(|os_str| os_str.to_string_lossy().to_string())
                    .unwrap_or(constants::FALLBACK_FILENAME.into());

                res.headers_mut().insert(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename={}", name).parse().unwrap(),
                );

                res
            }

            Err(err) => {
                log::warn!(
                    "Could not deliver file registered {} -> {}. {}",
                    path,
                    file_path,
                    err
                );

                self.not_found_page()
            }
        }
    }

    fn not_found_page(&self) -> Response<Body> {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap()
    }

    /// Run DirectShare
    pub async fn run(self: Arc<Self>, addr: SocketAddr) -> Result<(), DirectShareError> {
        let server = Server::bind(&addr).serve(make_service_fn(|socket: &AddrStream| {
            let app = self.clone();
            let remote_addr = socket.remote_addr();
            async move {
                Ok::<_, hyper::Error>(service_fn(move |body| {
                    let app = app.clone();
                    app.response(remote_addr, body)
                }))
            }
        }));

        server.await?;

        Ok(())
    }
}

#[derive(Debug)]
pub enum DirectShareError {
    Server(hyper::Error),
}

impl From<hyper::Error> for DirectShareError {
    fn from(err: hyper::Error) -> Self {
        Self::Server(err)
    }
}

impl Display for DirectShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirectShareError::Server(err) => err.fmt(f),
        }
    }
}

impl Error for DirectShareError {}
