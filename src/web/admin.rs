use super::prelude::*;
use super::{AppState, normalize_pet_name};
use crate::db::entities::{pets, votes};
use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Form, Path, State};
use axum::response::Redirect;
use chrono::{Duration, NaiveDate, Utc};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
pub(crate) struct PetUpdateForm {
    enabled: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct PetCreateForm {
    name: String,
    enabled: Option<String>,
}

#[derive(Clone, Debug)]
struct AdminPetView {
    name: String,
    enabled: bool,
    chart_svg: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "admin.html")]
pub(crate) struct AdminTemplate {
    pets: Vec<AdminPetView>,
    start_label: String,
    end_label: String,
    has_pets: bool,
    base_domain: String,
}

pub(crate) async fn admin_handler(
    State(state): State<AppState>,
) -> Result<AdminTemplate, HttpetError> {
    let db = &state.db;
    let pets_list = pets::Entity::find()
        .order_by_asc(pets::Column::Name)
        .all(db.as_ref())
        .await?;

    let today = Utc::now().date_naive();
    let start_date = today - Duration::days(29);
    let votes_list = votes::Entity::find()
        .filter(votes::Column::VoteDate.between(start_date, today))
        .order_by_asc(votes::Column::VoteDate)
        .all(db.as_ref())
        .await?;

    let mut vote_map: HashMap<i32, HashMap<NaiveDate, i32>> = HashMap::new();
    for vote in votes_list {
        vote_map
            .entry(vote.pet_id)
            .or_default()
            .insert(vote.vote_date, vote.vote_count);
    }

    let date_labels: Vec<NaiveDate> = (0..30)
        .map(|offset| start_date + Duration::days(offset))
        .collect();

    let pets = pets_list
        .into_iter()
        .map(|pet| {
            let chart_svg = if pet.enabled {
                String::new()
            } else {
                let vote_counts = build_vote_series(&date_labels, vote_map.get(&pet.id));
                render_vote_chart(&vote_counts)
            };
            AdminPetView {
                name: pet.name,
                enabled: pet.enabled,
                chart_svg,
            }
        })
        .collect::<Vec<_>>();

    let start_label = date_labels.first().map(format_date).unwrap_or_default();
    let end_label = date_labels.last().map(format_date).unwrap_or_default();

    Ok(AdminTemplate {
        has_pets: !pets.is_empty(),
        pets,
        start_label,
        end_label,
        base_domain: state.base_domain.clone(),
    })
}

pub(crate) async fn update_pet_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Form(form): Form<PetUpdateForm>,
) -> Result<Redirect, HttpetError> {
    let name = normalize_pet_name(&name);

    let enabled = form.enabled.is_some();
    state.create_or_update_pet(&name, enabled).await?;
    Ok(Redirect::to("/admin/"))
}

pub(crate) async fn create_pet_handler(
    State(state): State<AppState>,
    Form(form): Form<PetCreateForm>,
) -> Result<Redirect, HttpetError> {
    let name = normalize_pet_name(&form.name);
    if name.is_empty() {
        return Err(HttpetError::BadRequest);
    }

    let enabled = form.enabled.is_some();
    state.create_or_update_pet(&name, enabled).await?;

    Ok(Redirect::to("/admin/"))
}

fn build_vote_series(dates: &[NaiveDate], votes: Option<&HashMap<NaiveDate, i32>>) -> Vec<i32> {
    dates
        .iter()
        .map(|date| votes.and_then(|map| map.get(date).copied()).unwrap_or(0))
        .collect()
}

fn render_vote_chart(counts: &[i32]) -> String {
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
        r##"<svg viewBox="0 0 {width} {height}" preserveAspectRatio="none" role="img" aria-label="Votes over time">
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
