use crate::web::prelude::*;

#[derive(Deserialize)]
pub(crate) struct VoteForm {
    name: String,
}

pub(crate) async fn vote_form_handler(
    State(state): State<AppState>,
    Form(form): Form<VoteForm>,
) -> Result<VoteThanksTemplate, HttpetError> {
    let name = normalize_pet_name(&form.name);
    if name.is_empty() {
        return Err(HttpetError::BadRequest);
    }
    record_vote(&state.db, &name).await?;
    Ok(VoteThanksTemplate { name })
}

#[derive(Template, WebTemplate)]
#[template(path = "vote_page.html")]
pub(crate) struct VotePageTemplate {
    pub(crate) name: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "vote_thanks.html")]
pub(crate) struct VoteThanksTemplate {
    pub(crate) name: String,
}
