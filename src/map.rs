/*
 * Created on Sun Feb 06 2022
 *
 * Copyright (c) storycraft. Licensed under the MIT Licence.
 */

use std::{collections::HashMap, num::NonZeroU8, path::PathBuf};

use rand::{thread_rng, Rng};

#[derive(Debug, Clone)]
pub struct PathMap {
    key_length: NonZeroU8,
    map: HashMap<String, PathBuf>,
}

impl PathMap {
    pub fn new(key_length: NonZeroU8) -> Self {
        Self {
            key_length,
            map: HashMap::new(),
        }
    }

    /// Get file path from shorten uri
    pub fn get(&self, path: &str) -> Option<&PathBuf> {
        self.map.get(path)
    }

    /// Register new path and return path
    pub fn register(&mut self, path: PathBuf) -> String {
        let key = gen_key(self.key_length.get() as usize);

        self.map.insert(key.clone(), path);

        key
    }
}

fn gen_key(size: usize) -> String {
    const LIST: [char; 64] = [
        '_', '-', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
        'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x',
        'y', 'z', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P',
        'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
    ];

    let mut key = String::with_capacity(size);

    let mut rng = thread_rng();
    for _ in 0..size {
        key.push(LIST[rng.gen_range(0..64)]);
    }

    key
}

#[cfg(test)]
mod tests {
    use crate::map::gen_key;

    #[test]
    pub fn gen_key_test() {
        let key = gen_key(21);

        println!("{}", key);

        assert_eq!(key.len(), 21)
    }
}
