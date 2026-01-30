use askama::Template;

#[derive(Template)]
#[template(path = "vote_page.html")]
pub(crate) struct VotePageTemplate {
    pub(crate) name: String,
    pub(crate) status_code: u16,
}

#[derive(Template)]
#[template(path = "vote_thanks.html")]
pub(crate) struct VoteThanksTemplate {
    pub(crate) name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct HomePet {
    pub(crate) name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TopPet {
    pub(crate) name: String,
    pub(crate) votes: i64,
}

#[derive(Template)]
#[template(path = "home.html")]
pub(crate) struct HomeTemplate {
    pub(crate) enabled_pets: Vec<HomePet>,
    pub(crate) top_pets: Vec<TopPet>,
}
