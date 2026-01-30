use std::collections::HashMap;

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{Html, Redirect};
use chrono::{Duration, NaiveDate, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::db::entities::{pets, votes};

use super::AppState;

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

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate {
    pets: Vec<AdminPetView>,
    start_label: String,
    end_label: String,
    has_pets: bool,
}

pub(crate) async fn admin_handler(
    State(state): State<AppState>,
) -> Result<Html<String>, StatusCode> {
    let db = &state.db;
    let pets_list = pets::Entity::find()
        .order_by_asc(pets::Column::Name)
        .all(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let today = Utc::now().date_naive();
    let start_date = today - Duration::days(29);
    let votes_list = votes::Entity::find()
        .filter(votes::Column::VoteDate.between(start_date, today))
        .order_by_asc(votes::Column::VoteDate)
        .all(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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
            let vote_counts = build_vote_series(&date_labels, vote_map.get(&pet.id));
            let chart_svg = render_vote_chart(&vote_counts);
            AdminPetView {
                name: pet.name,
                enabled: pet.enabled,
                chart_svg,
            }
        })
        .collect::<Vec<_>>();

    let start_label = date_labels
        .first()
        .map(format_date)
        .unwrap_or_default();
    let end_label = date_labels
        .last()
        .map(format_date)
        .unwrap_or_default();

    let template = AdminTemplate {
        has_pets: !pets.is_empty(),
        pets,
        start_label,
        end_label,
    };

    let html = template
        .render()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Html(html))
}

pub(crate) async fn update_pet_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Form(form): Form<PetUpdateForm>,
) -> Result<Redirect, StatusCode> {
    let enabled = form.enabled.is_some();

    let db = &state.db;
    let existing = pets::Entity::find()
        .filter(pets::Column::Name.eq(&name))
        .one(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match existing {
        Some(model) => {
            let mut active: pets::ActiveModel = model.into();
            active.enabled = Set(enabled);
            active
                .update(db)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        None => {
            let active = pets::ActiveModel {
                name: Set(name.clone()),
                enabled: Set(enabled),
                ..Default::default()
            };
            active
                .insert(db)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
    }

    let mut enabled_pets = state.enabled_pets.write().await;
    if enabled {
        if !enabled_pets.contains(&name) {
            enabled_pets.push(name);
        }
    } else {
        enabled_pets.retain(|entry| entry != &name);
    }

    Ok(Redirect::to("/admin/"))
}

pub(crate) async fn create_pet_handler(
    State(state): State<AppState>,
    Form(form): Form<PetCreateForm>,
) -> Result<Redirect, StatusCode> {
    let name = form.name.trim().to_string();
    if name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let enabled = form.enabled.is_some();

    let db = &state.db;
    let existing = pets::Entity::find()
        .filter(pets::Column::Name.eq(&name))
        .one(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match existing {
        Some(model) => {
            let mut active: pets::ActiveModel = model.into();
            active.enabled = Set(enabled);
            active
                .update(db)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        None => {
            let active = pets::ActiveModel {
                name: Set(name.clone()),
                enabled: Set(enabled),
                ..Default::default()
            };
            active
                .insert(db)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
    }

    let mut enabled_pets = state.enabled_pets.write().await;
    if enabled {
        if !enabled_pets.contains(&name) {
            enabled_pets.push(name);
        }
    } else {
        enabled_pets.retain(|entry| entry != &name);
    }

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

fn format_date(date: &NaiveDate) -> String {
    date.format("%b %d").to_string()
}
