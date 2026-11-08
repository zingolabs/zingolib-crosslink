//! `LightClient` saves internally when it gets to a checkpoint. If has filesystem access, it saves to file at those points. otherwise, it passes the save buffer to the FFI.

use futures::FutureExt as _;
use log::error;

use std::{borrow::BorrowMut as _, fs::remove_file, sync::atomic};

use super::LightClient;
use crate::data::PollReport;

/// Writes `bytes` to file at `wallet_path`.
///
/// The bytes go to a `.tmp` sibling first, which is synced and then renamed over `wallet_path`,
/// so an interrupted write leaves the previous file intact.
fn write_to_path(wallet_path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp_wallet_path: std::path::PathBuf = wallet_path.with_extension(
        wallet_path
            .extension()
            .map(|e| format!("{}.tmp", e.to_string_lossy()))
            .unwrap_or_else(|| "tmp".to_string()),
    );
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp_wallet_path)?;
    let mut writer = std::io::BufWriter::new(file);
    std::io::Write::write_all(&mut writer, bytes)?;

    let file = writer.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    std::fs::rename(&temp_wallet_path, wallet_path)?;

    // NOTE: in windows no need to sync the folder, only for linux & macOS.
    #[cfg(unix)]
    {
        if let Some(parent) = wallet_path.parent() {
            let wallet_dir = std::fs::File::open(parent)?;
            let _ignore_error = wallet_dir.sync_all(); // NOTE: error is ignored as syncing dirs on windows OS may return an error
        }
    }

    Ok(())
}
impl LightClient {
    /// Serializes the wallet and writes it atomically to the configured wallet path when the
    /// wallet's `save_required` flag is set.
    ///
    /// Returns `Ok(true)` if a file was written and `Ok(false)` if nothing changed since the last
    /// save.
    pub async fn save_to_disk(&self) -> std::io::Result<bool> {
        let wallet_path = self.config.get_wallet_path();
        match self.wallet.write().await.save()? {
            Some(wallet_bytes) => {
                write_to_path(&wallet_path, &wallet_bytes)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Launches a task for saving the wallet data to persistance when the wallet's `save_required` flag is set.
    pub async fn save_task(&mut self) {
        if self.save_active.load(atomic::Ordering::Acquire) {
            return;
        }

        self.save_active.store(true, atomic::Ordering::Release);
        let save_active = self.save_active.clone();
        let wallet = self.wallet.clone();
        let wallet_path = self.config.get_wallet_path();
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let save_handle = tokio::spawn(async move {
            loop {
                interval.tick().await;
                if let Some(wallet_bytes) = wallet.write().await.save()? {
                    write_to_path(&wallet_path, &wallet_bytes)?;
                }
                if !save_active.load(atomic::Ordering::Acquire) {
                    return Ok(());
                }
            }
        });
        self.save_handle = Some(save_handle);
    }

    /// Wait until the wallet's `save_required` flag is not set.
    pub async fn wait_for_save(&self) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if !self.wallet.read().await.save_required {
                return;
            }
        }
    }

    /// Polls the save task, returning [`self::PollReport`].
    fn poll_save_task(&mut self) -> PollReport<(), std::io::Error> {
        if let Some(mut save_handle) = self.save_handle.take() {
            if let Some(save_result) = save_handle.borrow_mut().now_or_never() {
                PollReport::Ready(save_result.expect("task panicked"))
            } else {
                self.save_handle = Some(save_handle);
                PollReport::NotReady
            }
        } else {
            PollReport::NoHandle
        }
    }

    /// Checks the save task handle in case of failure.
    /// On save task failure, restarts the save task and returns the error.
    pub async fn check_save_error(&mut self) -> std::io::Result<()> {
        match self.poll_save_task() {
            PollReport::Ready(save_result) => {
                if save_result.is_err() {
                    self.save_task().await;
                }
                save_result
            }
            _ => Ok(()),
        }
    }

    pub async fn shutdown_save_task(&mut self) -> std::io::Result<()> {
        self.save_active.store(false, atomic::Ordering::Release);
        if let Some(save_handle) = self.save_handle.take() {
            save_handle.await.expect("task panicked")
        } else {
            Ok(())
        }
    }

    /// Only relevant in non-mobile, this function removes the save file.
    // TodO: can we shred it?
    pub async fn do_delete(&self) -> Result<(), String> {
        // Check if the file exists before attempting to delete
        if self.config.wallet_path_exists() {
            match remove_file(self.config.get_wallet_path()) {
                Ok(()) => {
                    log::debug!("File deleted successfully!");
                    Ok(())
                }
                Err(e) => {
                    let err = format!("ERR: {e}");
                    error!("{err}");
                    log::debug!("DELETE FAIL ON FILE!");
                    Err(e.to_string())
                }
            }
        } else {
            let err = "Error: File does not exist, nothing to delete.".to_string();
            error!("{err}");
            log::debug!("File does not exist, nothing to delete.");
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use bip0039::Mnemonic;
    use tempfile::TempDir;
    use zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED;

    use super::write_to_path;
    use crate::{
        config::{ChainType, ConfiguredActivationHeights, ZingoConfig},
        lightclient::{LightClient, error::LightClientError},
        wallet::{LightWallet, SeedDerivation, WalletBase},
    };

    fn regtest_config(temp_dir: &TempDir) -> ZingoConfig {
        ZingoConfig::build(ChainType::Regtest(
            ConfiguredActivationHeights::regtest_all_active(),
        ))
        .set_wallet_dir(temp_dir.path().to_path_buf())
        .create()
    }

    fn client_from_phrase(config: &ZingoConfig) -> LightClient {
        LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::Mnemonic {
                    mnemonic: Mnemonic::from_phrase(CHIMNEY_BETTER_SEED.to_string()).unwrap(),
                    no_of_accounts: config.no_of_accounts,
                    seed_derivation: SeedDerivation::Zip32Standard,
                },
                1.into(),
                config.wallet_settings.clone(),
            )
            .unwrap(),
            config.clone(),
            false,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn save_to_disk_round_trips_through_open_from_wallet_path() {
        let temp_dir = TempDir::new().unwrap();
        let config = regtest_config(&temp_dir);
        let lc = client_from_phrase(&config);

        assert!(lc.save_to_disk().await.unwrap());
        assert!(!lc.wallet.read().await.save_required);

        let (reopened, shape) = LightClient::open_from_wallet_path(config).unwrap();
        assert_eq!(shape, crate::wallet::disk::WalletFileShape::Raw);
        assert_eq!(
            "tmYd5GP6JxUxTUcz98NLPumEotvaMPaXytz".to_string(),
            reopened.transparent_addresses_json().await[0]["encoded_address"]
        );
        assert_eq!(
            "uregtest15en5x5cnsc7ye3wfy0prnh3ut34ns9w40htunlh9htfl6k5p004ja5gprxfz8fygjeax07a8489wzjk8gsx65thcp6d3ku8umgaka6f0"
                .to_string(),
            reopened.unified_addresses_json().await[0]["encoded_address"]
        );
    }

    #[tokio::test]
    async fn save_to_disk_is_noop_when_nothing_changed() {
        let temp_dir = TempDir::new().unwrap();
        let config = regtest_config(&temp_dir);
        let lc = client_from_phrase(&config);

        assert!(lc.save_to_disk().await.unwrap());
        let first = std::fs::read(config.get_wallet_path()).unwrap();

        assert!(!lc.save_to_disk().await.unwrap());
        let second = std::fs::read(config.get_wallet_path()).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn write_to_path_uses_tmp_sibling_and_removes_it() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");
        let tmp_path = temp_dir.path().join("wallet.dat.tmp");

        write_to_path(&wallet_path, b"A").unwrap();

        assert_eq!(std::fs::read(&wallet_path).unwrap(), b"A");
        assert!(!tmp_path.exists());
        let names: Vec<_> = std::fs::read_dir(temp_dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("wallet.dat")]);
    }

    #[test]
    fn write_to_path_leaves_previous_file_intact_on_failure() {
        let temp_dir = TempDir::new().unwrap();
        let wallet_path = temp_dir.path().join("wallet.dat");
        write_to_path(&wallet_path, b"A").unwrap();

        // A directory at the `.tmp` sibling's path cannot be opened for writing, so the second
        // write must fail before the rename.
        let tmp_path = temp_dir.path().join("wallet.dat.tmp");
        std::fs::create_dir(&tmp_path).unwrap();

        write_to_path(&wallet_path, b"B").unwrap_err();

        assert_eq!(std::fs::read(&wallet_path).unwrap(), b"A");
    }

    #[test]
    fn open_from_wallet_path_missing_file_is_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let config = regtest_config(&temp_dir);

        let error = LightClient::open_from_wallet_path(config).unwrap_err();

        match error {
            LightClientError::FileError(io_error) => {
                assert_eq!(io_error.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected FileError, got {other:?}"),
        }
    }
}
