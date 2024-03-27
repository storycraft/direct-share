/*
 * Created on Sat Feb 05 2022
 *
 * Copyright (c) storycraft. Licensed under the MIT Licence.
 */

pub mod config;
pub mod constants;
pub mod map;

use std::{
    convert::Infallible,
    env,
    error::Error,
    ffi::OsString,
    fs::Metadata,
    io::{self, ErrorKind},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    path::Path,
    sync::Arc,
};

use config::DirectShareConfig;
use constants::{FILE_BUF_SIZE, TAR_BUF_SIZE};
use futures_util::{FutureExt, TryStreamExt};
use http_body_util::{combinators::BoxBody, BodyExt, Empty, StreamBody};
use hyper::{
    body::{Bytes, Frame},
    header,
    server::conn::http1,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use log::LevelFilter;
use thiserror::Error;
use tokio::{
    fs::{self, File},
    io::duplex,
    net::TcpListener,
    spawn,
};
use tokio_util::io::ReaderStream;

use crate::map::PathMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::formatted_timed_builder()
        .filter_level({
            #[cfg(not(debug_assertions))]
            {
                LevelFilter::Info
            }
            #[cfg(debug_assertions)]
            {
                LevelFilter::Trace
            }
        })
        .init();

    log::info!("initializing DirectShare...");

    let config = match load_config().await {
        Ok(config) => config,

        Err(ConfigLoadError::Unreadable(err)) => {
            log::warn!("config is unreadable. using default config. {err}");

            let config = DirectShareConfig::default();

            if err.kind() == ErrorKind::NotFound {
                log::info!("Creating default config...");
                if let Err(write_err) = fs::write(
                    constants::CONFIG_FILE,
                    toml::to_string_pretty(&config).unwrap(),
                )
                .await
                {
                    log::warn!("cannot write default config err: {write_err}");
                } else {
                    log::info!("default config written");
                }
            }

            config
        }

        Err(ConfigLoadError::Invalid(err)) => {
            log::error!("config is corrupted or not in right format, please fix or delete config file and restart err: {err}");
            return Ok(());
        }
    };

    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if args.is_empty() {
        log::error!("please drag files to start server");
        return Ok(());
    }

    let mut map = PathMap::new(config.key_length);

    let ip = public_ip::addr()
        .await
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    for arg in args {
        let key = map.register(arg.clone().into());

        log::info!(
            "registered {} url: http://{ip}:{}/{key}",
            arg.to_string_lossy(),
            config.port
        );
    }

    log::info!("server starting on http://{}:{}/", ip, config.port);
    let listener = match TcpListener::bind(SocketAddrV4::new(
        Ipv4Addr::UNSPECIFIED,
        config.port.get(),
    ))
    .await
    {
        Ok(listener) => listener,
        Err(err) => {
            log::error!("cannot start server err: {err}");
            return Ok(());
        }
    };

    let map = Arc::new(map);
    loop {
        let (stream, addr) = listener.accept().await?;

        log::trace!("{addr} connected");

        spawn({
            let map = map.clone();

            async move {
                if let Err(err) = http1::Builder::new()
                    .serve_connection(
                        TokioIo::new(stream),
                        service_fn(|req| response(addr, &map, req).map(Ok::<_, Infallible>)),
                    )
                    .await
                {
                    log::error!("could not deliver file from addr: {addr} err: {err}");
                }
            }
        });
    }
}

async fn response(
    addr: SocketAddr,
    map: &PathMap,
    req: Request<hyper::body::Incoming>,
) -> Response<BoxBody<Bytes, io::Error>> {
    let method = req.method();
    let path = {
        let mut chars = req.uri().path().chars();
        chars.next();

        chars.as_str()
    };

    log::info!("method: {method} path: {path} addr: {addr}");

    if Method::GET != method {
        return not_found_page();
    }

    let Some(file_path) = map.get(path) else {
        return not_found_page();
    };

    let meta = match fs::metadata(file_path).await {
        Ok(meta) => meta,
        Err(err) => {
            log::error!("cannot stat {} err: {err}", file_path.display());

            return not_found_page();
        }
    };

    let file_name = file_path
        .file_name()
        .map(|os_str| os_str.to_string_lossy().to_string())
        .unwrap_or(constants::FALLBACK_FILENAME.into());

    if meta.is_file() {
        log::info!("serving file: {} addr: {addr}", file_path.display());
        serve_file(file_path.as_path(), &file_name, meta, req).await
    } else {
        log::info!("serving directory: {} addr: {addr}", file_path.display());
        serve_directory(file_path.as_path(), &file_name, req).await
    }
}

async fn serve_file(
    path: &Path,
    file_name: &str,
    meta: Metadata,
    _req: Request<hyper::body::Incoming>,
) -> Response<BoxBody<Bytes, io::Error>> {
    let file = match File::open(path).await {
        Ok(file) => file,
        Err(err) => {
            log::error!("cannot open file path: {} err: {err}", path.display());
            return not_found_page();
        }
    };

    let mut res = Response::new(
        StreamBody::new(ReaderStream::with_capacity(file, FILE_BUF_SIZE).map_ok(Frame::data))
            .boxed(),
    );

    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_LENGTH,
        meta.len().to_string().parse().unwrap(),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename={}", file_name)
            .parse()
            .unwrap(),
    );

    res
}

async fn serve_directory(
    path: &Path,
    dir_name: &str,
    _req: Request<hyper::body::Incoming>,
) -> Response<BoxBody<Bytes, io::Error>> {
    let archive_name = format!("{dir_name}.tar");

    let (tx, rx) = duplex(TAR_BUF_SIZE);

    tokio::spawn({
        let path = path.to_path_buf();

        async move {
            let mut ar = tokio_tar::Builder::new(tx);
            ar.append_dir_all(".", path).await?;
            ar.finish().await?;

            Ok::<_, io::Error>(())
        }
    });

    let mut res = Response::new(StreamBody::new(ReaderStream::new(rx).map_ok(Frame::data)).boxed());

    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename={}", archive_name)
            .parse()
            .unwrap(),
    );

    res
}

fn not_found_page() -> Response<BoxBody<Bytes, io::Error>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Empty::<Bytes>::new().map_err(|_| unreachable!()).boxed())
        .unwrap()
}

async fn load_config() -> Result<DirectShareConfig, ConfigLoadError> {
    let data = fs::read_to_string(constants::CONFIG_FILE)
        .await
        .map_err(ConfigLoadError::Unreadable)?;

    toml::from_str::<DirectShareConfig>(&data).map_err(ConfigLoadError::Invalid)
}

#[derive(Debug, Error)]
pub enum ConfigLoadError {
    #[error(transparent)]
    Invalid(#[from] toml::de::Error),
    #[error(transparent)]
    Unreadable(#[from] io::Error),
}
