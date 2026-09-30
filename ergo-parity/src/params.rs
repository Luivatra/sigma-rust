//! Blockchain parameters from an epoch-start block extension.
//!
//! Mirrors `Parameters.parseExtension` (ergo-core): every extension field with
//! key `[0x00, id]`, `id != 124` (soft-fork disabling rules, a variable-length
//! blob), carries a 4-byte big-endian `i32`.

use std::collections::BTreeMap;

use anyhow::{bail, Result};

/// Extension key prefix of system parameters (`Extension.SystemParametersPrefix`).
pub const SYSTEM_PARAMETERS_PREFIX: u8 = 0x00;
/// `Parameters.SoftForkDisablingRules`.
pub const SOFT_FORK_DISABLING_RULES: u8 = 124;
/// Voting epoch length on mainnet.
pub const VOTING_EPOCH_LENGTH: u32 = 1024;

/// Height of the block whose extension holds the parameters in force at `height`.
pub fn epoch_start(height: u32) -> u32 {
    height - height % VOTING_EPOCH_LENGTH
}

pub fn parse_parameters(fields: &[(Vec<u8>, Vec<u8>)]) -> Result<BTreeMap<u8, i32>> {
    let mut table = BTreeMap::new();
    for (k, v) in fields {
        if k.len() != 2 {
            bail!("wrong key length {} in extension field", k.len());
        }
        if k[0] == SYSTEM_PARAMETERS_PREFIX && k[1] != SOFT_FORK_DISABLING_RULES {
            let bytes: [u8; 4] = v.as_slice().try_into().map_err(|_| {
                anyhow::anyhow!("parameter {} value is {} bytes, expected 4", k[1], v.len())
            })?;
            table.insert(k[1], i32::from_be_bytes(bytes));
        }
    }
    if table.is_empty() {
        bail!("parameters table is empty");
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(k: &str, v: &str) -> (Vec<u8>, Vec<u8>) {
        (base16::decode(k).unwrap(), base16::decode(v).unwrap())
    }

    #[test]
    fn parses_system_parameters_and_ignores_others() {
        let fields = vec![
            f("0001", "0013d620"),
            f("0002", "00000168"),
            f("007b", "00000003"),
            f("007c", "0102030405"), // soft-fork disabling rules: skipped
            f("0100", "01b0244dfc"), // interlinks: not a parameter
        ];
        let p = parse_parameters(&fields).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[&1], 1_300_000);
        assert_eq!(p[&2], 360);
        assert_eq!(p[&123], 3);
    }

    #[test]
    fn negative_values_are_signed() {
        let p = parse_parameters(&[f("0001", "ffffffff")]).unwrap();
        assert_eq!(p[&1], -1);
    }

    #[test]
    fn rejects_bad_key_length_bad_value_length_and_empty() {
        assert!(parse_parameters(&[f("000102", "00000001")]).is_err());
        assert!(parse_parameters(&[f("0001", "0000")]).is_err());
        assert!(parse_parameters(&[f("0100", "00")]).is_err());
        assert!(parse_parameters(&[]).is_err());
    }

    #[test]
    fn epoch_start_is_multiple_of_1024() {
        assert_eq!(epoch_start(700_000), 699_392);
        assert_eq!(epoch_start(1024), 1024);
        assert_eq!(epoch_start(1023), 0);
    }
}
