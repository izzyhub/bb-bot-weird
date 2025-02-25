use thiserror::Error;
use defmt::{error, Format};
use alloc::string::ToString;

#[derive(Error, Debug, Format)]
pub enum BBBotError {
    #[error("Network Error")]
    NetworkError,
    #[error("Unit error")]
    UnitError,
    #[error("serde Error")]
    SerdeError(#[from] serde_json_core::de::Error),
    #[error("Version error")]
    VersionError,
}

impl From<reqwless::Error> for BBBotError {
    fn from(error: reqwless::Error) -> Self {
        error!("network error: {}", error);
        BBBotError::NetworkError
    }
}
impl From<()> for BBBotError {
    fn from(_: ()) -> Self {
        error!("unit error");
        BBBotError::UnitError
    }

}
impl From<semver::Error> for BBBotError {
    fn from(error: semver::Error)-> Self {
        let error_message = error.to_string();
        error!("semver error: {=str}", error_message);
        BBBotError::VersionError
    }
}
