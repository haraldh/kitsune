use crate::{
    error::Result, http::extractor::MastodonAuthExtractor, mapping::MastodonMapper,
    service::post::PostService, state::Zustand,
};
use axum::{
    debug_handler,
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::TryStreamExt;
use kitsune_type::mastodon::status::Context;
use std::collections::VecDeque;
use uuid::Uuid;

#[debug_handler(state = Zustand)]
pub async fn get(
    State(mastodon_mapper): State<MastodonMapper>,
    State(post): State<PostService>,
    user_data: Option<MastodonAuthExtractor>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let account_id = user_data.map(|user_data| user_data.0.account.id);

    let ancestors = post
        .get_ancestors(id, account_id)
        .try_fold(VecDeque::new(), |mut acc, item| async {
            acc.push_front(mastodon_mapper.map(item).await?);
            Ok(acc)
        })
        .await?;

    let descendants = post
        .get_descendants(id, account_id)
        .and_then(|post| mastodon_mapper.map(post))
        .try_collect()
        .await?;

    Ok(Json(Context {
        ancestors,
        descendants,
    })
    .into_response())
}