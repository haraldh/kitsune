use crate::{
    cache::Cacher,
    consts::USER_AGENT,
    db::model::{media_attachment, post, user},
    error::{Error, Result},
    sanitize::CleanHtmlExt,
};
use chrono::Utc;
use http::{HeaderMap, HeaderValue};
use phenomenon_model::ap::object::{Actor, Note};
use reqwest::Client;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
};
use std::time::Duration;
use url::Url;
use uuid::Uuid;

const CACHE_DURATION: Duration = Duration::from_secs(60); // 1 minute

#[derive(Clone)]
pub struct Fetcher {
    client: Client,
    db_conn: DatabaseConnection,

    // Caches
    post_cache: Cacher<str, post::Model>,
    user_cache: Cacher<str, user::Model>,
}

impl Fetcher {
    #[allow(clippy::missing_panics_doc)] // Invariants are covered. Won't panic.
    #[must_use]
    pub fn new(db_conn: DatabaseConnection, redis_conn: deadpool_redis::Pool) -> Self {
        let mut default_headers = HeaderMap::new();
        default_headers.insert(
            "Accept",
            HeaderValue::from_static("application/activity+json"),
        );

        Self {
            client: Client::builder()
                .default_headers(default_headers)
                .user_agent(USER_AGENT)
                .build()
                .unwrap(),
            db_conn,

            post_cache: Cacher::new(redis_conn.clone(), "fetcher-post", CACHE_DURATION),
            user_cache: Cacher::new(redis_conn, "fetcher-actor", CACHE_DURATION),
        }
    }

    /// Fetch an ActivityPub actor
    ///
    /// # Panics
    ///
    /// - Panics if the URL doesn't contain a host section
    pub async fn fetch_actor(&self, url: &str) -> Result<user::Model> {
        if let Some(user) = self.user_cache.get(url).await? {
            return Ok(user);
        }

        if let Some(user) = user::Entity::find()
            .filter(user::Column::Url.eq(url))
            .one(&self.db_conn)
            .await?
        {
            return Ok(user);
        }

        let url = Url::parse(url)?;
        let mut actor: Actor = self.client.get(url.clone()).send().await?.json().await?;
        actor.clean_html();

        let avatar_id = if let Some(icon) = actor.icon {
            let media_attachment = media_attachment::Model {
                id: Uuid::new_v4(),
                content_type: icon.media_type,
                url: icon.url,
                created_at: Utc::now(),
            }
            .into_active_model()
            .insert(&self.db_conn)
            .await?;

            Some(media_attachment.id)
        } else {
            None
        };

        let header_id = if let Some(image) = actor.image {
            let media_attachment = media_attachment::Model {
                id: Uuid::new_v4(),
                content_type: image.media_type,
                url: image.url,
                created_at: Utc::now(),
            }
            .into_active_model()
            .insert(&self.db_conn)
            .await?;

            Some(media_attachment.id)
        } else {
            None
        };

        user::Model {
            id: Uuid::new_v4(),
            avatar_id,
            header_id,
            display_name: actor.name,
            note: actor.subject,
            username: actor.preferred_username,
            email: None,
            password: None,
            domain: Some(url.host_str().unwrap().into()),
            url: actor.rest.id,
            inbox_url: actor.inbox,
            public_key: Some(actor.public_key.public_key_pem),
            private_key: None,
            created_at: actor.rest.published_at,
            updated_at: Utc::now(),
        }
        .into_active_model()
        .insert(&self.db_conn)
        .await
        .map_err(Error::from)
    }

    pub async fn fetch_note(&self, url: &str) -> Result<post::Model> {
        if let Some(post) = self.post_cache.get(url).await? {
            return Ok(post);
        }

        if let Some(post) = post::Entity::find()
            .filter(post::Column::Url.eq(url))
            .one(&self.db_conn)
            .await?
        {
            return Ok(post);
        }

        let mut note: Note = self.client.get(url).send().await?.json().await?;
        note.clean_html();

        let user = self
            .fetch_actor(note.rest.attributed_to().ok_or(Error::MalformedApObject)?)
            .await?;

        post::Model {
            id: Uuid::new_v4(),
            user_id: user.id,
            subject: note.subject,
            content: note.content,
            url: note.rest.id,
            created_at: note.rest.published_at,
            updated_at: Utc::now(),
        }
        .into_active_model()
        .insert(&self.db_conn)
        .await
        .map_err(Error::from)
    }
}