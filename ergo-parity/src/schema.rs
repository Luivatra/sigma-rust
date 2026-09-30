//! Fixture and result records (see SCHEMA.md).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Fixture {
    pub schema: u32,
    pub height: u32,
    pub block_id: String,
    pub tx_index: u32,
    pub tx_id: String,
    pub tx: String,
    pub inputs: Vec<String>,
    pub data_inputs: Vec<String>,
    pub headers: Vec<String>,
    pub header: String,
    pub parameters: BTreeMap<u8, i32>,
    pub epoch_extension: Vec<[String; 2]>,
    pub notes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct InputResult {
    pub index: u32,
    pub ok: bool,
    pub cost: Option<u64>,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ResultRec {
    pub schema: u32,
    pub runner: String,
    pub runner_version: String,
    pub tx_id: String,
    pub height: u32,
    pub status: String,
    pub total_cost: Option<u64>,
    pub inputs: Vec<InputResult>,
    pub error_class: Option<String>,
    pub error: Option<String>,
}

/// One line of the extractor's reject log.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Reject {
    pub height: u32,
    pub block_id: Option<String>,
    pub tx_index: Option<u32>,
    pub tx_id: Option<String>,
    pub reason: String,
    pub detail: String,
}
