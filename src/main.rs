/*
 * Created on Sat Feb 05 2022
 *
 * Copyright (c) storycraft. Licensed under the MIT Licence.
 */

pub mod app;
pub mod config;
pub mod constants;
pub mod map;

use std::{
    env,
    error::Error,
    io::{self, ErrorKind},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use config::DirectShareConfig;
use tokio::fs;

use crate::app::DirectShare;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    init_logger();

    log::info!("Initializing DirectShare...");
    let config = match load_config().await {
        Ok(config) => config,

        Err(ConfigLoadError::Unreadable(err)) => {
            log::warn!("Config is unreadable. Using default config. {}", err);

            let config = DirectShareConfig::default();

            if err.kind() == ErrorKind::NotFound {
                log::info!("Creating default config...");
                if let Err(write_err) = fs::write(
                    constants::CONFIG_FILE,
                    toml::to_string_pretty(&config).unwrap(),
                )
                .await
                {
                    log::warn!("Cannot write default config. {}", write_err);
                } else {
                    log::info!("Default config written");
                }
            }

            config
        }

        Err(ConfigLoadError::Invalid(err)) => {
            log::error!("Config is corrupted or not in right format. Please fix or delete config file and restart. {}", err);
            return Ok(());
        }
    };

    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 1 {
        log::error!("Program started without any file added. Please drag files or add arguments to file to start server.");
        return Ok(());
    }

    let mut app = DirectShare::new(config.key_length);

    let ip = public_ip::addr()
        .await
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    for arg in args {
        let key = app.register(arg.clone());

        log::info!(
            "File added. {} -> http://{}:{}/{}",
            arg,
            ip,
            config.port,
            key
        );
    }

    log::info!("Server starting on http://{}:{}/", ip, config.port);

    if let Err(err) = Arc::new(app)
        .run(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            config.port.get(),
        ))
        .await
    {
        log::error!("Error while running server. {}", err);
    }

    Ok(())
}

fn init_logger() {
    let level: &str;
    #[cfg(not(debug_assertions))]
    {
        level = "info";
    }
    #[cfg(debug_assertions)]
    {
        level = "trace";
    }

    env::set_var("APP_LOG", level);
    pretty_env_logger::init_custom_env("APP_LOG");
}

async fn load_config() -> Result<DirectShareConfig, ConfigLoadError> {
    match fs::read(constants::CONFIG_FILE).await {
        Ok(data) => match toml::from_slice::<DirectShareConfig>(&data) {
            Ok(config) => Ok(config),
            Err(err) => Err(ConfigLoadError::Invalid(err)),
        },

        Err(err) => Err(ConfigLoadError::Unreadable(err)),
    }
}

#[derive(Debug)]
pub enum ConfigLoadError {
    Invalid(toml::de::Error),
    Unreadable(io::Error),
}
