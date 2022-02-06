/*
 * Created on Sat Feb 05 2022
 *
 * Copyright (c) storycraft. Licensed under the MIT Licence.
 */

use std::num::{NonZeroU16, NonZeroU8};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// App config
pub struct DirectShareConfig {
    /// Port that can be used to bind server
    pub port: NonZeroU16,

    /// Key length for shorten url
    pub key_length: NonZeroU8,

    /// File that will be used for 404 page
    pub default_file: Option<String>,
}

impl Default for DirectShareConfig {
    fn default() -> Self {
        Self {
            port: NonZeroU16::new(1024).unwrap(),
            key_length: NonZeroU8::new(8).unwrap(),
            default_file: None,
        }
    }
}
