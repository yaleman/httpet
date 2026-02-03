use super::csrf::{csrf_token, validate_csrf};
use super::flash;
use super::images::{
    ImageCacheHeaders, apply_cache_headers, is_not_modified, not_modified_response,
};
use super::prelude::*;
use crate::constants::X_HTTPET_ANIMAL;
use crate::db::entities::{pets, votes};
use crate::status_codes;
use axum::extract::{Form, Multipart, Path, State};
use axum::http::HeaderMap;
use axum::response::{Redirect, Response};
use chrono::{Duration, NaiveDate, Utc};
use sea_orm::sea_query::{Alias, Expr, Query};
use sea_orm::{ColumnTrait, DatabaseBackend, EntityTrait, QueryFilter, QueryOrder, StatementBuilder};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, ErrorKind};
use std::path::Path as StdPath;
use std::str::FromStr;
use tracing::{debug, instrument};

#[derive(Deserialize)]
pub(crate) struct PetUpdateForm {
    status: String,
}

#[derive(Deserialize)]
pub(crate) struct PetCreateForm {
    name: String,
    status: String,
}

#[derive(Deserialize)]
pub(crate) struct PetDeleteForm {
    csrf_token: String,
    delete_images: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct PetStatusPath {
    name: String,
    status_code: u16,
}

#[derive(Clone, Debug)]
struct AdminPetView {
    name: String,
    status_label: String,
    status_class: String,
    is_enabled: bool,
    chart_svg: String,
    vote_total: i64,
}

#[derive(Template, WebTemplate)]
#[template(path = "admin.html")]
pub(crate) struct AdminTemplate {
    pets: Vec<AdminPetView>,
    start_label: String,
    end_label: String,
    has_pets: bool,
    has_orphan_pets: bool,
    orphan_pets: Vec<String>,
    state: AppState,
    csrf_token: String,
    has_flash: bool,
    flash_message: String,
    flash_class: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "admin_pet.html")]
pub(crate) struct AdminPetTemplate {
    pet_name: String,
    public_url: String,
    available_codes: Vec<u16>,
    missing_codes: Vec<u16>,
    has_unknown_files: bool,
    unknown_files: Vec<String>,
    vote_total: i64,
    status_label: String,
    status_class: String,
    has_flash: bool,
    flash_message: String,
    flash_class: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "admin_upload.html")]
pub(crate) struct AdminUploadTemplate {
    pet_name: String,
    status_code: u16,
    status_name: String,
    status_summary: String,
    status_mdn_url: String,
    has_existing: bool,
    existing_image_url: String,
    csrf_token: String,
    has_flash: bool,
    flash_message: String,
    flash_class: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "admin_delete.html")]
pub(crate) struct DeletePetTemplate {
    pet_name: String,
    has_images: bool,
    image_files: Vec<String>,
    csrf_token: String,
    has_flash: bool,
    flash_message: String,
    flash_class: String,
}

pub(crate) async fn admin_handler(
    State(state): State<AppState>,
    session: Session,
) -> Result<AdminTemplate, HttpetError> {
    let today = Utc::now().date_naive();
    let start_date = today - Duration::days(29);

    let pet_db = pets::Entity::find()
        .order_by_asc(pets::Column::Name)
        .all(state.db.as_ref())
        .await?;
    let pet_names: HashSet<String> = pet_db.iter().map(|pet| pet.name.clone()).collect();

    let date_labels: Vec<NaiveDate> = (0..30)
        .map(|offset| start_date + Duration::days(offset))
        .collect();

    let mut pets: Vec<AdminPetView> = Vec::new();
    let total_query = Query::select()
        .from(votes::Entity)
        .column(votes::Column::PetId)
        .expr_as(Expr::col(votes::Column::VoteCount).sum(), Alias::new("total_votes"))
        .group_by_col(votes::Column::PetId)
        .to_owned();
    let total_stmt = StatementBuilder::build(&total_query, &DatabaseBackend::Sqlite);
    let total_rows = state.db.query_all(total_stmt).await?;
    let mut vote_totals: HashMap<i32, i64> = HashMap::new();
    for row in total_rows {
        let pet_id: i32 = row.try_get("", "pet_id")?;
        let total_votes: i64 = row.try_get("", "total_votes")?;
        vote_totals.insert(pet_id, total_votes);
    }
    let votes = votes::Entity::find()
        .filter(votes::Column::VoteDate.gte(start_date))
        .order_by_asc(votes::Column::VoteDate)
        .all(state.db.as_ref())
        .await?;

    for pet in pet_db {
        let chart_svg = if pet.status == pets::PetStatus::Enabled {
            String::new()
        } else {
            let pet_votes = votes
                .iter()
                .filter(|v| v.pet_id == pet.id)
                .map(|v| (v.vote_date, v.vote_count))
                .collect();
            let vote_counts = build_vote_series(&date_labels, Some(&pet_votes));
            render_vote_chart(&pet.name, &vote_counts)
        };
        pets.push(AdminPetView {
            name: pet.name,
            status_label: pet.status.to_string(),
            status_class: pet.status.to_string(),
            is_enabled: pet.status == pets::PetStatus::Enabled,
            chart_svg,
            vote_total: vote_totals.get(&pet.id).copied().unwrap_or(0),
        });
    }

    let start_label = date_labels.first().map(format_date).unwrap_or_default();
    let end_label = date_labels.last().map(format_date).unwrap_or_default();

    let image_dirs = list_image_dirs(&state.image_dir).await?;
    let mut orphan_pets = Vec::new();
    let mut seen = HashSet::new();
    for dir in image_dirs {
        let name = normalize_pet_name(&dir);
        if name.is_empty() || pet_names.contains(&name) {
            continue;
        }
        if seen.insert(name.clone()) {
            orphan_pets.push(name);
        }
    }
    orphan_pets.sort();

    let csrf_token = csrf_token(&session).await?;
    let flash = flash::take_flash_message(&session).await?;
    let (has_flash, flash_message, flash_class) = match flash {
        Some(message) => (true, message.text.to_string(), message.class.to_string()),
        None => (false, String::new(), String::new()),
    };
    Ok(AdminTemplate {
        has_pets: !pets.is_empty(),
        has_orphan_pets: !orphan_pets.is_empty(),
        orphan_pets,
        pets,
        start_label,
        end_label,
        state,
        csrf_token,
        has_flash,
        flash_message,
        flash_class,
    })
}

pub(crate) async fn admin_pet_view(
    State(state): State<AppState>,
    session: Session,
    Path(name): Path<String>,
) -> Result<AdminPetTemplate, HttpetError> {
    let pet_name = normalize_pet_name_strict(&name)?;

    let Some(pet) = pets::Entity::find_by_name(state.db.as_ref(), &pet_name).await? else {
        return Err(HttpetError::NotFound(pet_name));
    };

    let status_map = status_codes::status_codes()
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
    let known_codes: Vec<u16> = status_map.keys().copied().collect();
    let known_set: HashSet<u16> = known_codes.iter().copied().collect();

    let image_files = list_pet_images(&state.image_dir, &pet_name).await?;
    let mut available_codes = Vec::new();
    let mut unknown_files = Vec::new();
    for file in image_files {
        let code = StdPath::new(&file)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u16>().ok());
        if let Some(code) = code
            && known_set.contains(&code)
        {
            available_codes.push(code);
            continue;
        }
        unknown_files.push(file);
    }
    available_codes.sort_unstable();
    available_codes.dedup();
    let available_set: HashSet<u16> = available_codes.iter().copied().collect();
    let missing_codes: Vec<u16> = known_codes
        .iter()
        .copied()
        .filter(|code| !available_set.contains(code))
        .collect();

    let flash = flash::take_flash_message(&session).await?;
    let (has_flash, flash_message, flash_class) = match flash {
        Some(message) => (true, message.text.to_string(), message.class.to_string()),
        None => (false, String::new(), String::new()),
    };

    let total_query = Query::select()
        .from(votes::Entity)
        .expr_as(Expr::col(votes::Column::VoteCount).sum(), Alias::new("total_votes"))
        .and_where(Expr::col(votes::Column::PetId).eq(pet.id))
        .to_owned();
    let total_stmt = StatementBuilder::build(&total_query, &DatabaseBackend::Sqlite);
    let vote_total = match state.db.query_one(total_stmt).await? {
        Some(row) => row.try_get("", "total_votes").unwrap_or(0),
        None => 0,
    };

    Ok(AdminPetTemplate {
        pet_name: pet_name.clone(),
        public_url: state.pet_base_url(&pet_name),
        available_codes,
        missing_codes,
        has_unknown_files: !unknown_files.is_empty(),
        unknown_files,
        vote_total,
        status_label: pet.status.to_string(),
        status_class: pet.status.to_string(),
        has_flash,
        flash_message,
        flash_class,
    })
}

pub(crate) async fn admin_pet_upload_view(
    State(state): State<AppState>,
    session: Session,
    Path(path): Path<PetStatusPath>,
) -> Result<AdminUploadTemplate, HttpetError> {
    let pet_name = normalize_pet_name_strict(&path.name)?;
    if !(100..=599).contains(&path.status_code) {
        return Err(HttpetError::BadRequest);
    }

    let pet_exists = pets::Entity::find_by_name(state.db.as_ref(), &pet_name)
        .await?
        .is_some();
    if !pet_exists {
        return Err(HttpetError::NotFound(pet_name));
    }

    let status_map = status_codes::status_codes()
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
    let Some(info) = status_map.get(&path.status_code) else {
        return Err(HttpetError::NotFound(path.status_code.to_string()));
    };

    let image_path = state.image_path(&pet_name, path.status_code);
    let has_existing = match tokio::fs::metadata(&image_path).await {
        Ok(metadata) => metadata.is_file(),
        Err(err) if err.kind() == ErrorKind::NotFound => false,
        Err(err) => return Err(HttpetError::InternalServerError(err.to_string())),
    };

    let csrf_token = csrf_token(&session).await?;
    let flash = flash::take_flash_message(&session).await?;
    let (has_flash, flash_message, flash_class) = match flash {
        Some(message) => (true, message.text.to_string(), message.class.to_string()),
        None => (false, String::new(), String::new()),
    };

    Ok(AdminUploadTemplate {
        pet_name: pet_name.clone(),
        status_code: path.status_code,
        status_name: info.name.clone(),
        status_summary: info.summary.clone(),
        status_mdn_url: info.mdn_url.clone(),
        has_existing,
        existing_image_url: format!("/admin/pets/{}/images/{}", pet_name, path.status_code),
        csrf_token,
        has_flash,
        flash_message,
        flash_class,
    })
}

pub(crate) async fn admin_pet_image_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(path): Path<PetStatusPath>,
) -> Result<Response, HttpetError> {
    let pet_name = normalize_pet_name_strict(&path.name)?;
    if !(100..=599).contains(&path.status_code) {
        return Err(HttpetError::BadRequest);
    }

    let image_path = state.image_path(&pet_name, path.status_code);
    let metadata = match tokio::fs::metadata(&image_path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(HttpetError::NotFound(format!(
                "{} {}",
                pet_name, path.status_code
            )));
        }
        Err(err) => return Err(HttpetError::InternalServerError(err.to_string())),
    };
    let cache_headers = ImageCacheHeaders::from_metadata(&metadata);
    if is_not_modified(&headers, &cache_headers) {
        return not_modified_response(&cache_headers);
    }
    let mut builder = Response::builder();
    match tokio::fs::read(&image_path).await {
        Ok(bytes) => {
            if let Ok(value) = HeaderValue::from_str(&pet_name) {
                builder = builder.header(X_HTTPET_ANIMAL, value);
            }
            builder = builder.header(CONTENT_TYPE, "image/jpeg");
            builder = apply_cache_headers(builder, &cache_headers);
            builder
                .body(axum::body::Body::from(bytes))
                .map_err(HttpetError::from)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Err(HttpetError::NotFound(format!(
            "{} {}",
            pet_name, path.status_code
        ))),
        Err(err) => Err(HttpetError::InternalServerError(err.to_string())),
    }
}

pub(crate) async fn delete_pet_view(
    State(state): State<AppState>,
    session: Session,
    Path(name): Path<String>,
) -> Result<DeletePetTemplate, HttpetError> {
    let pet_name = normalize_pet_name_strict(&name)?;

    let pet_exists = pets::Entity::find_by_name(state.db.as_ref(), &pet_name)
        .await?
        .is_some();
    if !pet_exists {
        return Err(HttpetError::NotFound(pet_name));
    }

    let image_files = list_pet_images(&state.image_dir, &pet_name).await?;
    let has_images = !image_files.is_empty();
    let csrf_token = csrf_token(&session).await?;
    let flash = flash::take_flash_message(&session).await?;
    let (has_flash, flash_message, flash_class) = match flash {
        Some(message) => (true, message.text.to_string(), message.class.to_string()),
        None => (false, String::new(), String::new()),
    };

    Ok(DeletePetTemplate {
        pet_name,
        has_images,
        image_files,
        csrf_token,
        has_flash,
        flash_message,
        flash_class,
    })
}

pub(crate) async fn update_pet_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Form(form): Form<PetUpdateForm>,
) -> Result<Redirect, HttpetError> {
    let name = normalize_pet_name_strict(&name)?;

    let status_value = form.status.trim().to_ascii_lowercase();
    let status =
        pets::PetStatus::from_str(status_value.as_str()).map_err(|_| HttpetError::BadRequest)?;
    state.create_or_update_pet(&name, status).await?;
    Ok(Redirect::to("/admin/"))
}

#[instrument(skip_all, fields(name = %form.name, status = %form.status))]
pub(crate) async fn create_pet_handler(
    State(state): State<AppState>,
    Form(form): Form<PetCreateForm>,
) -> Result<Redirect, HttpetError> {
    let name = normalize_pet_name_strict(&form.name)?;

    let status_value = form.status.trim().to_ascii_lowercase();
    let status =
        pets::PetStatus::from_str(status_value.as_str()).map_err(|_| HttpetError::BadRequest)?;
    state.create_or_update_pet(&name, status).await?;

    Ok(Redirect::to("/admin/"))
}

pub(crate) async fn upload_image_handler(
    State(state): State<AppState>,
    session: Session,
    mut multipart: Multipart,
) -> Result<Redirect, HttpetError> {
    let mut pet_name: Option<String> = None;
    let mut status_code: Option<u16> = None;
    let mut image_bytes: Option<Vec<u8>> = None;
    let mut csrf_token_value: Option<String> = None;
    let mut redirect_to: Option<String> = None;
    let mut overwrite: bool = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?
    {
        let field_name = field.name().unwrap_or_default();
        match field_name {
            "pet" => {
                let name = field
                    .text()
                    .await
                    .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
                pet_name = Some(normalize_pet_name_strict(&name)?);
            }
            "csrf_token" => {
                let value = field
                    .text()
                    .await
                    .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
                csrf_token_value = Some(value);
            }
            "status_code" => {
                let code = field
                    .text()
                    .await
                    .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
                let parsed = code.parse::<u16>().map_err(|_| HttpetError::BadRequest)?;
                if !(100..=599).contains(&parsed) {
                    return Err(HttpetError::BadRequest);
                }
                status_code = Some(parsed);
            }
            "image" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
                image_bytes = Some(bytes.to_vec());
            }
            "redirect_to" => {
                let value = field
                    .text()
                    .await
                    .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
                redirect_to = Some(value);
            }
            "overwrite" => {
                overwrite = true;
            }
            _ => {}
        }
    }

    let pet_name = pet_name
        .filter(|name| !name.is_empty())
        .ok_or(HttpetError::BadRequest)?;
    let status_code = status_code.ok_or(HttpetError::BadRequest)?;
    let image_bytes = image_bytes.ok_or(HttpetError::BadRequest)?;
    let csrf_token_value = csrf_token_value.ok_or(HttpetError::BadRequest)?;
    validate_csrf(&session, &csrf_token_value).await?;
    let image_bytes = normalize_image_to_jpeg(&image_bytes)?;

    let pet_exists = pets::Entity::find_by_name(state.db.as_ref(), &pet_name)
        .await?
        .is_some();
    if !pet_exists {
        return Err(HttpetError::BadRequest);
    }

    let pet_dir = state.image_dir.join(&pet_name);
    tokio::fs::create_dir_all(&pet_dir)
        .await
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
    let image_path = pet_dir.join(format!("{status_code}.jpg"));
    let exists = match tokio::fs::metadata(&image_path).await {
        Ok(metadata) => metadata.is_file(),
        Err(err) if err.kind() == ErrorKind::NotFound => false,
        Err(err) => return Err(HttpetError::InternalServerError(err.to_string())),
    };
    if exists && !overwrite {
        flash::set_flash(&session, flash::FLASH_OVERWRITE_REQUIRED).await?;
        return Ok(Redirect::to(&format!(
            "/admin/pets/{}/status/{}",
            pet_name, status_code
        )));
    }
    tokio::fs::write(&image_path, image_bytes)
        .await
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;

    flash::set_flash(&session, flash::FLASH_UPLOAD_SUCCESS).await?;
    let redirect_target = redirect_to
        .as_deref()
        .filter(|target| target.starts_with("/admin/"))
        .unwrap_or("/admin/");
    Ok(Redirect::to(redirect_target))
}

/// Deletes a pet and its images
#[instrument(skip_all, fields(name = %name, delete_images=?form.delete_images))]
pub(crate) async fn delete_pet_post(
    State(state): State<AppState>,
    session: Session,
    Path(name): Path<String>,
    Form(form): Form<PetDeleteForm>,
) -> Result<Redirect, HttpetError> {
    validate_csrf(&session, &form.csrf_token).await?;

    let pet_name = normalize_pet_name_strict(&name)?;

    let image_files = list_pet_images(&state.image_dir, &pet_name).await?;
    if !image_files.is_empty() {
        if form.delete_images.is_none() {
            flash::set_flash(&session, flash::FLASH_DELETE_IMAGES_REQUIRED).await?;
            return Ok(Redirect::to(&format!("/admin/pets/{}/delete", pet_name)));
        }
        let pet_dir = state.image_dir.join(&pet_name);
        if let Err(err) = tokio::fs::remove_dir_all(&pet_dir).await
            && err.kind() != ErrorKind::NotFound
        {
            return Err(HttpetError::InternalServerError(err.to_string()));
        }
    }

    state.delete_pet(&pet_name).await?;
    Ok(Redirect::to("/admin/"))
}

/// Ensures image bytes are a valid JPEG, converting if possible.
fn normalize_image_to_jpeg(bytes: &[u8]) -> Result<Vec<u8>, HttpetError> {
    if bytes.len() < 4 {
        debug!("Image is too short");
        return Err(HttpetError::BadRequest);
    }

    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| {
            debug!("Failed to guess image format: {}", err);
            HttpetError::BadRequest
        })?;
    let format = reader.format();
    let image = reader.decode().map_err(|err| {
        debug!("Failed to decode image: {}", err);
        HttpetError::BadRequest
    })?;

    if format == Some(image::ImageFormat::Jpeg) {
        return Ok(bytes.to_vec());
    }

    let mut output = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut output);
    encoder
        .encode_image(&image)
        .map_err(|err| HttpetError::InternalServerError(err.to_string()))?;
    Ok(output)
}

/// zips the dates and votes into a series of vote counts
fn build_vote_series(dates: &[NaiveDate], votes: Option<&HashMap<NaiveDate, i32>>) -> Vec<i32> {
    dates
        .iter()
        .map(|date| votes.and_then(|map| map.get(date).copied()).unwrap_or(0))
        .collect()
}

async fn list_pet_images(image_dir: &StdPath, pet_name: &str) -> Result<Vec<String>, HttpetError> {
    let dir = image_dir.join(pet_name);
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(HttpetError::InternalServerError(err.to_string())),
    };

    let mut images = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let is_jpg = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("jpg"))
            .unwrap_or(false);
        if !is_jpg {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            images.push(name.to_string());
        }
    }
    images.sort();
    Ok(images)
}

async fn list_image_dirs(image_dir: &StdPath) -> Result<Vec<String>, HttpetError> {
    let mut entries = match tokio::fs::read_dir(image_dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(HttpetError::InternalServerError(err.to_string())),
    };

    let mut dirs = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            dirs.push(name.to_string());
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Turns the votes into an SVG chart
fn render_vote_chart(pet_name: &str, counts: &[i32]) -> String {
    let width = 720.0;
    let height = 180.0;
    let padding = 18.0;
    let max = counts.iter().copied().max().unwrap_or(0).max(1) as f32;
    let step_x = if counts.len() > 1 {
        (width - padding * 2.0) / (counts.len() as f32 - 1.0)
    } else {
        0.0
    };
    let points: Vec<String> = counts
        .iter()
        .enumerate()
        .map(|(idx, count)| {
            let x = padding + step_x * idx as f32;
            let y = height - padding - ((*count as f32) / max) * (height - padding * 2.0);
            format!("{:.1},{:.1}", x, y)
        })
        .collect();
    let polyline = points.join(" ");
    let area = format!(
        "{},{} {} {},{}",
        padding,
        height - padding,
        polyline,
        width - padding,
        height - padding
    );

    format!(
        r##"<svg class="vote-chart" viewBox="0 0 {width} {height}" preserveAspectRatio="none" role="img" aria-label="Votes over time for {pet_name}">
  <defs>
    <linearGradient id="voteGradient" x1="0" x2="0" y1="0" y2="1">
      <stop offset="0%" stop-color="#3b82f6" stop-opacity="0.35" />
      <stop offset="100%" stop-color="#3b82f6" stop-opacity="0.02" />
    </linearGradient>
  </defs>
  <rect x="0" y="0" width="{width}" height="{height}" fill="#f8fafc" rx="12" />
  <polyline points="{area}" fill="url(#voteGradient)" stroke="none" />
  <polyline points="{polyline}" fill="none" stroke="#1d4ed8" stroke-width="3" />
</svg>"##,
        width = width,
        height = height,
        area = area,
        polyline = polyline
    )
}

#[allow(clippy::trivially_copy_pass_by_ref)] // so that we can use it in map()
/// Formats a date as "Mon DD"
fn format_date(date: &NaiveDate) -> String {
    date.format("%b %d").to_string()
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_normalize_image_to_jpeg() {
        use crate::config::setup_logging;
        let _ = setup_logging(true);
        let jpeg_bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/images/dog/100.jpg"));
        let normalized = normalize_image_to_jpeg(jpeg_bytes).expect("normalize jpeg");
        assert_eq!(normalized, jpeg_bytes);
        assert!(normalize_image_to_jpeg(&[]).is_err());
        assert!(normalize_image_to_jpeg(&[0xFF, 0xD8, 0x00, 0xFF, 0xD9]).is_err());
        assert!(normalize_image_to_jpeg(b"This is not a JPEG file.").is_err());
    }
}
