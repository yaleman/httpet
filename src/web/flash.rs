use tower_sessions::Session;

use crate::error::HttpetError;

const FLASH_FLAG_KEY: &str = "flash_flag";

pub(crate) const FLASH_UPLOAD_SUCCESS: u16 = 1;
pub(crate) const FLASH_DELETE_IMAGES_REQUIRED: u16 = 2;
pub(crate) const FLASH_OVERWRITE_REQUIRED: u16 = 3;

#[derive(Clone, Debug)]
pub(crate) struct FlashMessage {
    pub(crate) text: &'static str,
    pub(crate) class: &'static str,
}

pub(crate) async fn set_flash(session: &Session, flag: u16) -> Result<(), HttpetError> {
    session
        .insert(FLASH_FLAG_KEY, flag)
        .await
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
    Ok(())
}

pub(crate) async fn take_flash_message(
    session: &Session,
) -> Result<Option<FlashMessage>, HttpetError> {
    let flag = session
        .get::<u16>(FLASH_FLAG_KEY)
        .await
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?
        .filter(|flag| *flag != 0);
    if flag.is_some() {
        session
            .insert(FLASH_FLAG_KEY, 0u16)
            .await
            .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
    }
    Ok(flag.and_then(message_for))
}

fn message_for(flag: u16) -> Option<FlashMessage> {
    match flag {
        FLASH_UPLOAD_SUCCESS => Some(FlashMessage {
            text: "Upload successful. Your image is now available.",
            class: "success",
        }),
        FLASH_DELETE_IMAGES_REQUIRED => Some(FlashMessage {
            text: "Please confirm that you want to delete all images for this pet.",
            class: "warning",
        }),
        FLASH_OVERWRITE_REQUIRED => Some(FlashMessage {
            text: "An image already exists for this status. Confirm overwrite to continue.",
            class: "warning",
        }),
        _ => None,
    }
}
