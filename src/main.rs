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
    num::NonZeroU16,
    path::Path,
    sync::Arc,
    time::Duration,
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
use igd::{aio::search_gateway, PortMappingProtocol, SearchOptions};
use local_ip_address::local_ip;
use log::LevelFilter;
use never_say_never::Never;
use thiserror::Error;
use tokio::{
    fs::{self, File},
    io::duplex,
    net::TcpListener,
    select, signal, spawn,
    time::sleep,
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

    let config = load_config().await;

    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if args.is_empty() {
        log::error!("please drag files to start server");
        return Ok(());
    }

    let mut map = PathMap::new(config.key_length);

    let ip = local_ip().unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

    spawn(upnp_service(ip, config.port));

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

    select! {
        Ok(_) = signal::ctrl_c() => {
            log::info!("stopping server...");
        }
        _ = server(listener, Arc::new(map)) => {}
    };

    Ok(())
}

async fn server(listener: TcpListener, map: Arc<PathMap>) -> Result<Never, anyhow::Error> {
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
                    log::warn!("could not deliver file from addr: {addr} err: {err}");
                }
            }
        });
    }
}

async fn upnp_service(ip: IpAddr, port: NonZeroU16) {
    let gateway = match search_gateway(SearchOptions::default()).await {
        Ok(gateway) => gateway,
        Err(err) => {
            log::warn!("uPnP discovery failed err: {err}");
            return;
        }
    };

    if let Ok(external_ip) = gateway.get_external_ip().await {
        if ip != external_ip {
            log::warn!("NAT detected external_ip: {external_ip}");
            log::warn!("use {external_ip} instead when sharing over WAN");
        }
    }

    let IpAddr::V4(ip) = ip else {
        return;
    };

    let port = port.get();

    let task = async {
        const TIMEOUT: Duration = Duration::from_secs(120);

        'task_loop: loop {
            let mut attempts = 0;
            while let Err(err) = gateway
                .add_port(
                    PortMappingProtocol::TCP,
                    port,
                    SocketAddrV4::new(ip, port),
                    TIMEOUT.as_secs() as u32,
                    "DirectShare port mapping",
                )
                .await
            {
                if attempts >= 5 {
                    log::error!("uPnP port mapping failed, please do port forwarding manually or cannot be shared over WAN");
                    break 'task_loop;
                }

                let next = Duration::from_secs(5 + attempts * 5);
                log::warn!(
                    "uPnP port mapping failed, retrying after {} secs err: {err}",
                    next.as_secs()
                );

                sleep(next).await;
                attempts += 1;
            }

            sleep(TIMEOUT).await;
        }
    };

    let cleanup = async {
        if signal::ctrl_c().await.is_err() {
            log::warn!("SIGINT signal hook failed.");
            return;
        };

        let _ = gateway.remove_port(PortMappingProtocol::TCP, port).await;
    };

    select! {
        _ = task => {}
        _ = cleanup => {}
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

async fn load_config() -> DirectShareConfig {
    #[derive(Debug, Error)]
    pub enum Error {
        #[error(transparent)]
        Invalid(#[from] toml::de::Error),
        #[error(transparent)]
        Unreadable(#[from] io::Error),
    }

    async fn load() -> Result<DirectShareConfig, Error> {
        let data = fs::read_to_string(constants::CONFIG_FILE)
            .await
            .map_err(Error::Unreadable)?;

        toml::from_str::<DirectShareConfig>(&data).map_err(Error::Invalid)
    }

    match load().await {
        Ok(config) => config,

        Err(Error::Unreadable(err)) => {
            log::warn!("config is unreadable. using default config. {err}");

            let config = DirectShareConfig::default();

            if err.kind() == ErrorKind::NotFound {
                log::info!("creating default config...");
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

        Err(Error::Invalid(err)) => {
            log::error!(
                "config is corrupted or not in right format, using default config err: {err}"
            );

            DirectShareConfig::default()
        }
    }
}
