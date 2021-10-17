/* This Source Code Form is subject to the terms of the Mozilla Public
   License, v. 2.0. If a copy of the MPL was not distributed with this
   file, You can obtain one at http://mozilla.org/MPL/2.0/.
*/

//! Account-related B2 API calls.
// TODO: Timestamps are likely UTC. Is that documented anywhere?

// TODO: export these from another/a sub module.
// crate::file_data? data_types? file_metadata?
// The ContentDisposition type defined in this module needs to go with them.
pub use http_types::{
    cache::{CacheDirective, Expires},
    content::ContentEncoding,
    mime::Mime,
};

use std::fmt;

use crate::{
    client::HttpClient,
    error::{B2Error, ValidationError, Error},
};

use chrono::{DateTime, Local};
use serde::{Serialize, Deserialize};


const B2_AUTH_URL: &str = if cfg!(test) {
    "http://localhost:8765/b2api/v2/"
} else {
    "https://api.backblazeb2.com/b2api/v2/"
};

// This gives us nicer error handling when deserializing JSON responses.
#[derive(Deserialize)]
#[serde(untagged)]
enum B2Result<T> {
    Ok(T),
    Err(B2Error),
}

/// Authorization token and related information obtained from
/// [authorize_account].
///
/// The token is valid for no more than 24 hours.
// TODO: We probably do need to make this serializable.
#[derive(Debug)]
pub struct Authorization<C>
    where C: HttpClient,
{
    pub(crate) client: C,
    account_id: String,
    // The authorization token to use for all future API calls.
    //
    // The token is valid for no more than 24 hours.
    authorization_token: String,
    allowed: Capabilities,
    // The base URL for all API calls except uploading or downloading files.
    api_url: String,
    // The base URL to use for downloading files.
    download_url: String,
    recommended_part_size: u64,
    absolute_minimum_part_size: u64,
    // The base URL to use for all API calls using the AWS S3-compatible API.
    s3_api_url: String,
}

impl<C> Authorization<C>
    where C: HttpClient,
{
    /// The ID for the account.
    pub fn account_id(&self) -> &str { &self.account_id }
    /// The capabilities granted to this auth token.
    pub fn capabilities(&self) -> &Capabilities { &self.allowed }
    /// The recommended size in bytes for each part of a large file.
    pub fn recommended_part_size(&self) -> u64 { self.recommended_part_size }
    /// The smallest possible size in bytes of a part of a large file, except
    /// the final part.
    pub fn minimum_part_size(&self) -> u64 { self.absolute_minimum_part_size }
}

impl<C> Authorization<C>
    where C: HttpClient,
{
    /// Return the API url to the specified service endpoint.
    ///
    /// This URL is used for all API calls except downloading files.
    pub(crate) fn api_url<S: AsRef<str>>(&self, endpoint: S) -> String {
        format!("{}/b2api/v2/{}", self.api_url, endpoint.as_ref())
    }

    /// Return the API url to the specified service download endpoint.
    pub(crate) fn download_url<S: AsRef<str>>(&self, endpoint: S) -> String {
        format!("{}/b2api/v2/{}", self.download_url, endpoint.as_ref())
    }

    /// Return the API url to the specified S3-compatible service download
    /// endpoint.
    pub(crate) fn s3_api_url<S: AsRef<str>>(&self, endpoint: S) -> String {
        format!("{}/b2api/v2/{}", self.s3_api_url, endpoint.as_ref())
    }
}

/// The authorization information received from B2
///
/// The public [Authorization] object contains everything here, plus private
/// data used by this API implementation, such as the HTTP client.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtoAuthorization {
    account_id: String,
    authorization_token: String,
    allowed: Capabilities,
    api_url: String,
    download_url: String,
    recommended_part_size: u64,
    absolute_minimum_part_size: u64,
    s3_api_url: String,
}

impl ProtoAuthorization {
    fn create_authorization<C: HttpClient>(self, c: C) -> Authorization<C> {
        Authorization {
            client: c,
            account_id: self.account_id,
            authorization_token: self.authorization_token,
            allowed: self.allowed,
            api_url: self.api_url,
            download_url: self.download_url,
            recommended_part_size: self.recommended_part_size,
            absolute_minimum_part_size: self.absolute_minimum_part_size,
            s3_api_url: self.s3_api_url,
        }
    }
}

/// The set of capabilities and associated information granted by an
/// authorization token.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    capabilities: Vec<Capability>,
    bucket_id: Option<String>,
    bucket_name: Option<String>,
    name_prefix: Option<String>,
}

impl Capabilities {
    /// The list of capabilities granted.
    pub fn capabilities(&self) -> &[Capability] { &self.capabilities }
    /// If the capabilities are limited to a single bucket, this is the bucket's
    /// ID.
    pub fn bucket_id(&self) -> Option<&String> { self.bucket_id.as_ref() }
    /// If the bucket is valid and hasn't been deleted, the name of the bucket
    /// corresponding to `bucket_id`. If the bucket referred to by `bucket_id`
    /// no longer exists, this will be `None`.
    pub fn bucket_name(&self) -> Option<&String> { self.bucket_name.as_ref() }
    /// If set, access is limited to files whose names begin with this prefix.
    pub fn name_prefix(&self) -> Option<&String> { self.name_prefix.as_ref() }

    /// Check if the provided capability is granted to the object containing
    /// this [Capabilities] object.
    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities.iter().any(|&c| c == cap)
    }
}

/// A capability potentially granted by an authorization token.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    ListKeys,
    WriteKeys,
    DeleteKeys,
    ListAllBucketNames,
    ListBuckets,
    ReadBuckets,
    WriteBuckets,
    DeleteBuckets,
    ReadBucketRetentions,
    WriteBucketRetentions,
    ReadBucketEncryption,
    WriteBucketEncryption,
    ListFiles,
    ReadFiles,
    ShareFiles,
    WriteFiles,
    DeleteFiles,
    ReadFileLegalHolds,
    WriteFileLegalHolds,
    ReadFileRetentions,
    WriteFileRetentions,
    BypassGovernance,
}

/// Log onto the B2 API.
///
/// The returned [Authorization] object must be passed to subsequent API calls.
///
/// You can obtain the `key_id` and `key` from the B2 administration pages or
/// from [create_key].
///
/// See <https://www.backblaze.com/b2/docs/b2_authorize_account.html> for
/// further information.
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "with_surf")]
/// # use b2_client::{
/// #     client::{HttpClient, SurfClient},
/// #     account::{authorize_account, delete_key_by_id},
/// # };
/// # #[cfg(feature = "with_surf")]
/// # async fn f() -> anyhow::Result<()> {
/// let mut auth = authorize_account(SurfClient::new(), "MY KEY ID", "MY KEY")
///     .await?;
///
/// let removed_key = delete_key_by_id(&mut auth, "OTHER KEY ID").await?;
/// # Ok(()) }
/// ```
pub async fn authorize_account<C, E>(mut client: C, key_id: &str, key: &str)
-> Result<Authorization<C>, Error<E>>
    where C: HttpClient<Response=serde_json::Value, Error=Error<E>>,
          E: fmt::Debug + fmt::Display,
{
    let id_and_key = format!("{}:{}", key_id, key);
    let id_and_key = base64::encode(id_and_key.as_bytes());

    let mut auth = String::from("Basic ");
    auth.push_str(&id_and_key);

    let req = client.get(
        format!("{}b2_authorize_account", B2_AUTH_URL)
    ).expect("Invalid URL")
        .with_header("Authorization", &auth);

    let res = req.send().await?;

    let auth: B2Result<ProtoAuthorization> = serde_json::from_value(res)?;
    match auth {
        B2Result::Ok(r) => Ok(r.create_authorization(client)),
        B2Result::Err(e) => Err(Error::B2(e)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Duration(chrono::Duration);

impl std::ops::Deref for Duration {
    type Target = chrono::Duration;

    fn deref(&self) -> &Self::Target { &self.0 }
}

impl From<chrono::Duration> for Duration {
    fn from(d: chrono::Duration) -> Self {
        Self(d)
    }
}

impl From<Duration> for chrono::Duration {
    fn from(d: Duration) -> Self { d.0 }
}

impl Serialize for Duration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where S: serde::Serializer,
    {
        serializer.serialize_i64(self.num_milliseconds())
    }
}

struct DurationVisitor;

impl<'de> serde::de::Visitor<'de> for DurationVisitor {
    type Value = i64;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "the number of milliseconds representing the duration"
        )
    }

    fn visit_i64<E>(self, s: i64) -> Result<Self::Value, E>
        where E: serde::de::Error,
    {
        Ok(s)
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D>(deserializer: D) -> Result<Duration, D::Error>
        where D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_i64(DurationVisitor)
            .map(|i| Duration(chrono::Duration::milliseconds(i)))
    }
}


/// A request to create a B2 API key with certain capabilities.
///
/// Use [CreateKeyRequestBuilder] to create a `CreateKeyRequest` object, then
/// pass it to [create_key] to create a new application [Key] from the request.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateKeyRequest {
    // account_id is provided by the Authorization object.
    account_id: Option<String>,
    capabilities: Vec<Capability>,
    key_name: String,
    valid_duration_in_seconds: Option<Duration>,
    bucket_id: Option<String>,
    name_prefix: Option<String>,
}

/// A builder to create a [CreateKeyRequest] object.
///
/// After creating the `CreateKeyRequest`, pass it to [create_key] to obtain a
/// new application key.
///
/// See <https://www.backblaze.com/b2/docs/b2_create_key.html> for more
/// information.
pub struct CreateKeyRequestBuilder {
    capabilities: Option<Vec<Capability>>,
    name: String,
    valid_duration: Option<Duration>,
    bucket_id: Option<String>,
    name_prefix: Option<String>,
}

impl CreateKeyRequestBuilder {
    /// Create a new builder, with the key's name provided.
    pub fn new<S: Into<String>>(name: S) -> Result<Self, ValidationError> {
        // TODO: Name must be ASCII?
        let name = name.into();

        if name.len() > 100 {
            return Err(ValidationError::Invalid(
                "Name must be no more than 100 characters.".into()
            ));
        }

        let invalid_char = |c: &char| !(c.is_alphanumeric() || *c == '-');

        if let Some(ch) = name.chars().find(invalid_char) {
            return Err(
                ValidationError::Invalid(format!("Invalid character: {}", ch))
            );
        }

        Ok(Self {
            capabilities: None,
            name,
            valid_duration: None,
            bucket_id: None,
            name_prefix: None,
        })
    }

    /// Create the key with the specified capabilities.
    ///
    /// At least one capability must be provided.
    pub fn with_capabilities<V: Into<Vec<Capability>>>(mut self, caps: V)
    -> Result<Self, ValidationError> {
        let caps = caps.into();

        if caps.is_empty() {
            return Err(ValidationError::Invalid(
                "Key must have at least one capability.".into()
            ));
        }

        self.capabilities = Some(caps);
        Ok(self)
    }

    /// Set an expiration duration for the key.
    ///
    /// If provided, the key must be positive and no more than 1,000 days.
    pub fn expires_after(mut self, dur: chrono::Duration)
    -> Result<Self, ValidationError> {
        if dur >= chrono::Duration::days(1000) {
            return Err(ValidationError::Invalid(
                "Expiration must be less than 1000 days".into()
            ));
        } else if dur < chrono::Duration::seconds(1) {
            return Err(ValidationError::Invalid(
                "Expiration must be a positive number of seconds".into()
            ));
        }

        self.valid_duration = Some(Duration(dur));
        Ok(self)
    }

    /// Limit the key's access to the specified bucket.
    pub fn limit_to_bucket<S: Into<String>>(mut self, id: S)
    -> Result<Self, ValidationError> {
        let id = id.into();
        // TODO: Validate bucket id.

        self.bucket_id = Some(id);
        Ok(self)
    }

    /// Limit access to files to those that begin with the specified prefix.
    pub fn with_name_prefix<S: Into<String>>(mut self, prefix: S)
    -> Result<Self, ValidationError> {
        let prefix = prefix.into();
        // TODO: Validate prefix

        self.name_prefix = Some(prefix);
        Ok(self)
    }

    /// Create a new [CreateKeyRequest].
    pub fn build(self) -> Result<CreateKeyRequest, ValidationError> {
        let capabilities = self.capabilities.ok_or_else(||
            ValidationError::Invalid(
                "A list of capabilities for the key is required.".into()
            )
        )?;

        if self.bucket_id.is_some() {
            for cap in &capabilities {
                match cap {
                    Capability::ListAllBucketNames
                    | Capability::ListBuckets
                    | Capability::ReadBuckets
                    | Capability::ReadBucketEncryption
                    | Capability::WriteBucketEncryption
                    | Capability::ReadBucketRetentions
                    | Capability::WriteBucketRetentions
                    | Capability::ListFiles
                    | Capability::ReadFiles
                    | Capability::ShareFiles
                    | Capability::WriteFiles
                    | Capability::DeleteFiles
                    | Capability::ReadFileLegalHolds
                    | Capability::WriteFileLegalHolds
                    | Capability::ReadFileRetentions
                    | Capability::WriteFileRetentions
                    | Capability::BypassGovernance => {},
                    cap => return Err(ValidationError::Invalid(format!(
                        "Invalid capability when bucket_id is set: {:?}",
                        cap
                    ))),
                }
            }
        } else if self.name_prefix.is_some() {
            return Err(ValidationError::Invalid(
                "bucket_id must be set when name_prefix is given".into()
            ));
        }

        Ok(CreateKeyRequest {
            account_id: None,
            capabilities,
            key_name: self.name,
            valid_duration_in_seconds: self.valid_duration,
            bucket_id: self.bucket_id,
            name_prefix: self.name_prefix,
        })
    }
}

/// An application key and associated information.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    key_name: String,
    application_key_id: String,
    capabilities: Vec<Capability>,
    account_id: String,
    expiration_timestamp: Option<DateTime<Local>>,
    bucket_id: Option<String>,
    name_prefix: Option<String>,
    options: Option<Vec<String>>, // Currently unused by B2.
}

impl Key {
    /// The name assigned to this key.
    pub fn key_name(&self) -> &str { &self.key_name }
    /// The list of capabilities granted by this key.
    pub fn capabilities(&self) -> &[Capability] { &self.capabilities }
    /// The account this key is for.
    pub fn account_id(&self) -> &str { &self.account_id }
    /// If present, this key's capabilities are restricted to the returned
    /// bucket.
    pub fn bucket_id(&self) -> Option<&String> { self.bucket_id.as_ref() }
    /// If set, access is limited to files whose names begin with this prefix.
    pub fn name_prefix(&self) -> Option<&String> { self.name_prefix.as_ref() }

    /// If present, the expiration date and time of this key.
    pub fn expiration(&self) -> Option<DateTime<Local>> {
        self.expiration_timestamp
    }

    /// Check if the provided capability is granted by this key.
    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities.iter().any(|&c| c == cap)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewlyCreatedKey {
    // The private part of the key. This is only returned upon key creation, so
    // must be stored in a safe place.
    application_key: String,

    // The rest of these are part of (and moved to) the Key.
    key_name: String,
    application_key_id: String,
    capabilities: Vec<Capability>,
    account_id: String,
    expiration_timestamp: Option<DateTime<Local>>,
    bucket_id: Option<String>,
    name_prefix: Option<String>,
    options: Option<Vec<String>>,
}

impl NewlyCreatedKey {
    fn create_public_key(self) -> (String, Key) {
        let secret = self.application_key;

        let key = Key {
            key_name: self.key_name,
            application_key_id: self.application_key_id,
            capabilities: self.capabilities,
            account_id: self.account_id,
            expiration_timestamp: self.expiration_timestamp,
            bucket_id: self.bucket_id,
            name_prefix: self.name_prefix,
            options: self.options,
        };

        (secret, key)
    }
}

/// Create a new API application key.
///
/// Returns a tuple of the key secret and the key capability information. The
/// secret is never obtainable except by this function, so must be stored in a
/// secure location.
///
/// See <https://www.backblaze.com/b2/docs/b2_create_key.html> for further
/// information.
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "with_surf")]
/// # use b2_client::{
/// #     client::{HttpClient, SurfClient},
/// #     account::{
/// #         authorize_account, create_key,
/// #         Capability, CreateKeyRequestBuilder,
/// #     },
/// # };
/// # #[cfg(feature = "with_surf")]
/// # async fn f() -> anyhow::Result<()> {
/// let mut auth = authorize_account(SurfClient::new(), "MY KEY ID", "MY KEY")
///     .await?;
///
/// let create_key_request = CreateKeyRequestBuilder::new("my-key")?
///     .with_capabilities([Capability::ListFiles])?
///     .build()?;
///
/// let (secret, new_key) = create_key(&mut auth, create_key_request).await?;
/// # Ok(()) }
/// ```
pub async fn create_key<C, E>(
    auth: &mut Authorization<C>,
    new_key_info: CreateKeyRequest
) -> Result<(String, Key), Error<E>>
    where C: HttpClient<Response=serde_json::Value, Error=Error<E>>,
          E: fmt::Debug + fmt::Display,
{
    let mut new_key_info = new_key_info;
    new_key_info.account_id = Some(auth.account_id.to_owned());

    let res = auth.client.post(auth.api_url("b2_create_key"))
        .expect("Invalid URL")
        .with_header("Authorization", &auth.authorization_token)
        .with_body(&serde_json::to_value(new_key_info)?)
        .send().await?;

    let new_key: B2Result<NewlyCreatedKey> = serde_json::from_value(res)?;
    match new_key {
        B2Result::Ok(key) => Ok(key.create_public_key()),
        B2Result::Err(e) => Err(Error::B2(e)),
    }
}

/// Delete the given [Key].
///
/// Returns a `Key` describing the just-deleted key.
///
/// See <https://www.backblaze.com/b2/docs/b2_delete_key.html> for further
/// information.
///
/// ```no_run
/// # #[cfg(feature = "with_surf")]
/// # use b2_client::{
/// #     client::{HttpClient, SurfClient},
/// #     account::{
/// #         authorize_account, create_key, delete_key,
/// #         Capability, CreateKeyRequestBuilder,
/// #     },
/// # };
/// # #[cfg(feature = "with_surf")]
/// # async fn f() -> anyhow::Result<()> {
/// let mut auth = authorize_account(SurfClient::new(), "MY KEY ID", "MY KEY")
///     .await?;
///
/// let create_key_request = CreateKeyRequestBuilder::new("my-key")?
///     .with_capabilities([Capability::ListFiles])?
///     .build()?;
///
/// let (_secret, new_key) = create_key(&mut auth, create_key_request).await?;
///
/// let deleted_key = delete_key(&mut auth, new_key).await?;
/// # Ok(()) }
/// ```
pub async fn delete_key<C, E>(auth: &mut Authorization<C>, key: Key)
-> Result<Key, Error<E>>
    where C: HttpClient<Response=serde_json::Value, Error=Error<E>>,
          E: fmt::Debug + fmt::Display,
{
    delete_key_by_id(auth, key.application_key_id).await
}

/// Delete the key with the specified key ID.
///
/// Returns a [Key] describing the just-deleted key.
///
/// See <https://www.backblaze.com/b2/docs/b2_delete_key.html> for further
/// information.
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "with_surf")]
/// # use b2_client::{
/// #     client::{HttpClient, SurfClient},
/// #     account::{authorize_account, delete_key_by_id},
/// # };
/// # #[cfg(feature = "with_surf")]
/// # async fn f() -> anyhow::Result<()> {
/// let mut auth = authorize_account(SurfClient::new(), "MY KEY ID", "MY KEY")
///     .await?;
///
/// let removed_key = delete_key_by_id(&mut auth, "OTHER KEY ID").await?;
/// # Ok(()) }
/// ```
pub async fn delete_key_by_id<C, E, S: AsRef<str>>(
    auth: &mut Authorization<C>,
    key_id: S
) -> Result<Key, Error<E>>
    where C: HttpClient<Response=serde_json::Value, Error=Error<E>>,
          E: fmt::Debug + fmt::Display,
{
    let res = auth.client.post(auth.api_url("b2_delete_key"))
        .expect("Invalid URL")
        .with_header("Authorization", &auth.authorization_token)
        .with_body(&serde_json::json!({"applicationKeyId": key_id.as_ref()}))
        .send().await?;

    let key: B2Result<Key> = serde_json::from_value(res)?;
    match key {
        B2Result::Ok(key) => Ok(key),
        B2Result::Err(e) => Err(Error::B2(e)),
    }
}

/// A Content-Disposition value.
///
/// The grammar is specified in RFC 6266, except parameter names that contain an
/// '*' are not allowed.
// TODO: Implement; parse/validate.
pub struct ContentDisposition(String);

/// A request to obtain a [DownloadAuthorization].
///
/// Use [DownloadAuthorizationRequestBuilder] to create a
/// `DownloadAuthorizationRequest`, then pass it to [get_download_authorization]
/// to obtain a [DownloadAuthorization].
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadAuthorizationRequest {
    bucket_id: String,
    file_name_prefix: String,
    valid_duration_in_seconds: Duration,
    b2_content_disposition: Option<String>,
    b2_content_language: Option<String>,
    b2_expires: Option<String>,
    // Storing and converting these will be expensive, but I don't want to try
    // manually implementing a non_exhaustive enum that I don't control.
    b2_cache_control: Option<String>,
    b2_content_encoding: Option<String>,
    b2_content_type: Option<String>,
}

/// A builder to create a [DownloadAuthorizationRequest].
///
/// After building the `DownloadAuthorizationRequest`, pass it to
/// [get_download_authorization] to obtain a [DownloadAuthorization]
///
/// The bucket ID, file name prefix, and valid duration are required.
///
/// See <https://www.backblaze.com/b2/docs/b2_get_download_authorization.html>
/// for furter information.
pub struct DownloadAuthorizationRequestBuilder {
    // Required:
    bucket_id: Option<String>,
    file_name_prefix: Option<String>,
    valid_duration_in_seconds: Option<Duration>,
    // Optional:
    b2_content_disposition: Option<String>,
    b2_content_language: Option<String>,
    b2_expires: Option<String>,
    b2_cache_control: Option<String>,
    b2_content_encoding: Option<String>,
    b2_content_type: Option<String>,
}

impl DownloadAuthorizationRequestBuilder {
    /// Create a new `DownloadAuthorizationRequestBuilder`.
    pub fn new() -> Self {
        Self {
            bucket_id: None,
            file_name_prefix: None,
            valid_duration_in_seconds: None,
            b2_content_disposition: None,
            b2_content_language: None,
            b2_expires: None,
            b2_cache_control: None,
            b2_content_encoding: None,
            b2_content_type: None,
        }
    }

    /// Create a download authorization for the specified bucket ID.
    pub fn for_bucket_id<S: Into<String>>(mut self, id: S) -> Self {
        // TODO: Validate id.
        self.bucket_id = Some(id.into());
        self
    }

    /// Use the given file name prefix to determine what files the
    /// [DownloadAuthorization] will allow access to.
    pub fn with_file_name_prefix<S: Into<String>>(mut self, name: S) -> Self {
        // TODO: Validate prefix.
        self.file_name_prefix = Some(name.into());
        self
    }

    /// Specify the amount of time for which the [DownloadAuthorization] will be
    /// valid.
    ///
    /// This must be between one second and one week, inclusive.
    pub fn with_duration(mut self, dur: chrono::Duration)
    -> Result<Self, ValidationError> {
        if dur < chrono::Duration::seconds(1)
            || dur > chrono::Duration::weeks(1)
        {
            return Err(ValidationError::Invalid(
                "Duration must be between 1 and 604,800 seconds, inclusive"
                    .into()
            ));
        }

        self.valid_duration_in_seconds = Some(Duration(dur));
        Ok(self)
    }

    /// If specified, download requests must have this content disposition. The
    /// grammar is specified in RFC 6266, except that parameter names containing
    /// a '*' are not allowed.
    pub fn with_content_disposition(mut self, disposition: ContentDisposition)
    -> Self {
        self.b2_content_disposition = Some(disposition.0);
        self
    }

    /// If specified, download requests must have this content language. The
    /// grammar is specified in RFC 2616.
    pub fn with_content_language<S: Into<String>>(mut self, lang: S) -> Self {
        // TODO: Validate language.
        self.b2_content_language = Some(lang.into());
        self
    }

    /// If specified, download requests must have this expiration.
    pub fn with_expiration(mut self, expiration: Expires) -> Self {
        self.b2_expires = Some(expiration.value().to_string());
        self
    }

    /// If specified, download requests must have this cache control.
    pub fn with_cache_control(mut self, directive: CacheDirective) -> Self {
        use http_types::headers::HeaderValue;

        self.b2_cache_control = Some(HeaderValue::from(directive).to_string());
        self
    }

    /// If specified, download requests must have this content encoding.
    pub fn with_content_encoding(mut self, encoding: ContentEncoding) -> Self {
        self.b2_content_encoding = Some(format!("{}", encoding.encoding()));
        self
    }

    /// If specified, download requests must have this content type.
    pub fn with_content_type(mut self, content_type: Mime) -> Self {
        self.b2_content_type = Some(content_type.to_string());
        self
    }

    /// Build a [DownloadAuthorizationRequest].
    pub fn build(self) -> Result<DownloadAuthorizationRequest, ValidationError>
    {
        let bucket_id = self.bucket_id
            .ok_or_else(|| ValidationError::Invalid(
                "A bucket ID must be provided".into()
            ))?;
        let file_name_prefix = self.file_name_prefix
            .ok_or_else(|| ValidationError::Invalid(
                "A filename prefix must be provided".into()
            ))?;
        let valid_duration_in_seconds = self.valid_duration_in_seconds
            .ok_or_else(|| ValidationError::Invalid(
                "The duration of the authorization token must be set".into()
            ))?;

        Ok(DownloadAuthorizationRequest {
            bucket_id,
            file_name_prefix,
            valid_duration_in_seconds,
            b2_content_disposition: self.b2_content_disposition,
            b2_content_language: self.b2_content_language,
            b2_expires: self.b2_expires,
            b2_cache_control: self.b2_cache_control,
            b2_content_encoding: self.b2_content_encoding,
            b2_content_type: self.b2_content_type,
        })
    }
}

/// A capability token that authorizes downloading files from a private bucket.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadAuthorization {
    bucket_id: String,
    file_name_prefix: String,
    authorization_token: String,
}

impl DownloadAuthorization {
    /// Get the ID of the bucket this `DownloadAuthorization` can access.
    pub fn bucket_id(&self) -> &str { &self.bucket_id }
    /// The file prefix that determines what files in the bucket are accessible
    /// via this `DownloadAuthorization`.
    pub fn file_name_prefix(&self) -> &str { &self.file_name_prefix }
}

/// Generate a download authorization token to download files with a specific
/// prefix from a private B2 bucket.
///
/// The [Authorization] token must have [Capability::ShareFiles].
///
/// The returned [DownloadAuthorization] can be passed to
/// [download_file_by_name].
///
/// See <https://www.backblaze.com/b2/docs/b2_get_download_authorization.html>
/// for further information.
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "with_surf")]
/// # use b2_client::{
/// #     client::{HttpClient, SurfClient},
/// #     account::{
/// #         authorize_account, get_download_authorization,
/// #         DownloadAuthorizationRequestBuilder,
/// #     },
/// # };
/// # #[cfg(feature = "with_surf")]
/// # async fn f() -> anyhow::Result<()> {
/// let mut auth = authorize_account(SurfClient::new(), "MY KEY ID", "MY KEY")
///     .await?;
///
/// let download_req = DownloadAuthorizationRequestBuilder::new()
///     .for_bucket_id("MY BUCKET ID")
///     .with_file_name_prefix("my/files/")
///     .with_duration(chrono::Duration::seconds(60))?
///     .build()?;
///
/// let download_auth = get_download_authorization(&mut auth, download_req)
///     .await?;
/// # Ok(()) }
/// ```
// TODO: Once download endpoints are implemented, add one to the example above.
pub async fn get_download_authorization<C, E>(
    auth: &mut Authorization<C>,
    download_req: DownloadAuthorizationRequest
) -> Result<DownloadAuthorization, Error<E>>
    where C: HttpClient<Response=serde_json::Value, Error=Error<E>>,
          E: fmt::Debug + fmt::Display,
{
    let res = auth.client.post(auth.api_url("b2_get_download_authorization"))
        .expect("Invalid URL")
        .with_header("Authorization", &auth.authorization_token)
        .with_body(&serde_json::to_value(download_req)?)
        .send().await?;

    let download_auth: B2Result<DownloadAuthorization>
        = serde_json::from_value(res)?;

    match download_auth {
        B2Result::Ok(auth) => Ok(auth),
        B2Result::Err(e) => Err(Error::B2(e)),
    }
}

// TODO: Find a good way to mock responses for any/all backends.
#[cfg(feature = "with_surf")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        client::SurfClient,
        error::ErrorCode,
    };
    use surf_vcr::{VcrMiddleware, VcrMode, VcrError};


    const AUTH_KEY_ID: &str = "B2_KEY_ID";
    const AUTH_KEY: &str = "B2_AUTH_KEY";

    /// Create a SurfClient with the surf-vcr middleware.
    async fn create_test_client(mode: VcrMode, cassette: &'static str)
    -> std::result::Result<SurfClient, VcrError> {
        let surf = surf::Client::new()
            .with(VcrMiddleware::new(mode, cassette).await?);

        let client = SurfClient::new()
            .with_client(surf);

        Ok(client)
    }

    /// Create a fake authorization to allow us to run tests without calling the
    /// authorize_account function.
    fn get_test_key(client: SurfClient, capabilities: Vec<Capability>)
    -> Authorization<SurfClient> {
        Authorization {
            client,
            account_id: "abcdefg".into(),
            authorization_token: "4_002d2e6b27577ea0000000002_019f9ac2_4af224_acct_BzTNBWOKUVQvIMyHK3tXHG7YqDQ=".into(),
            allowed: Capabilities {
                capabilities,
                bucket_id: None,
                bucket_name: None,
                name_prefix: None,
            },
            api_url: "http://localhost:8765".into(),
            download_url: "http://localhost:8765/download".into(),
            recommended_part_size: 100000000,
            absolute_minimum_part_size: 5000000,
            s3_api_url: "http://localhost:8765/s3api".into(),
        }
    }

    #[async_std::test]
    async fn test_authorize_account() -> Result<(), anyhow::Error> {
        let client = create_test_client(
            VcrMode::Replay,
            "test_sessions/auth_account.yaml"
        ).await?;

        let auth = authorize_account(client, AUTH_KEY_ID, AUTH_KEY).await?;
        assert!(auth.allowed.capabilities.contains(&Capability::ListBuckets));

        Ok(())
    }

    #[async_std::test]
    async fn authorize_account_bad_key() -> Result<(), anyhow::Error> {
        let client = create_test_client(
            VcrMode::Replay,
            "test_sessions/auth_account.yaml"
        ).await?;

        let auth = authorize_account(client, AUTH_KEY_ID, "wrong-key").await;

        match auth.unwrap_err() {
            // The B2 documentation says we'll receive `unauthorized`, but this
            // is what we get.
            Error::B2(e) => assert_eq!(e.code(), ErrorCode::BadAuthToken),
            _ => panic!("Unexpected error type"),
        }

        Ok(())
    }

    #[async_std::test]
    async fn authorize_account_bad_key_id() -> Result<(), anyhow::Error> {
        let client = create_test_client(
            VcrMode::Replay,
            "test_sessions/auth_account.yaml"
        ).await?;

        let auth = authorize_account(client, "wrong-id", AUTH_KEY).await;

        match auth.unwrap_err() {
            // The B2 documentation says we'll receive `unauthorized`, but this
            // is what we get.
            Error::B2(e) => assert_eq!(e.code(), ErrorCode::BadAuthToken),
            e => panic!("Unexpected error type: {:?}", e),
        }

        Ok(())
    }

    #[async_std::test]
    async fn test_create_key() -> Result<(), anyhow::Error> {
        let client = create_test_client(
            VcrMode::Replay,
            "test_sessions/auth_account.yaml"
        ).await?;

        let mut auth = get_test_key(client, vec![Capability::WriteKeys]);

        let new_key_info = CreateKeyRequestBuilder::new("my-special-key")
            .unwrap()
            .with_capabilities(vec![Capability::ListFiles]).unwrap()
            .build().unwrap();

        let (secret, key) = create_key(&mut auth, new_key_info).await?;
        assert!(! secret.is_empty());
        assert_eq!(key.capabilities.len(), 1);
        assert_eq!(key.capabilities[0], Capability::ListFiles);

        Ok(())
    }

    #[async_std::test]
    async fn test_delete_key() -> Result<(), anyhow::Error> {
        let client = create_test_client(
            VcrMode::Replay,
            "test_sessions/auth_account.yaml"
        ).await?;

        let mut auth = get_test_key(client, vec![Capability::DeleteKeys]);

        let removed_key = delete_key_by_id(
            &mut auth, "002d2e6b27577ea0000000005"
        ).await?;

        assert_eq!(removed_key.key_name, "my-special-key");

        Ok(())
    }

    #[async_std::test]
    async fn test_get_download_authorization() -> Result<(), anyhow::Error> {
        let client = create_test_client(
            VcrMode::Replay,
            "test_sessions/auth_account.yaml"
        ).await?;

        let mut auth = get_test_key(client, vec![Capability::ShareFiles]);

        let req = DownloadAuthorizationRequestBuilder::new()
            .for_bucket_id("8d625eb63be2775577c70e1a")
            .with_file_name_prefix("files/")
            .with_duration(chrono::Duration::seconds(30))?
            .with_content_disposition(
                ContentDisposition("Attachment; filename=example.html".into())
            )
            //.with_expiration(Expires::new(std::time::Duration::from_secs(60)))
            .with_cache_control(CacheDirective::MustRevalidate)
            .build()?;

        let download_auth = get_download_authorization(&mut auth, req).await?;
        assert_eq!(download_auth.bucket_id(), "8d625eb63be2775577c70e1a");

        Ok(())
    }

    #[async_std::test]
    async fn test_get_download_authorization_with_only_required_data()
    -> Result<(), anyhow::Error> {
        let client = create_test_client(
            VcrMode::Replay,
            "test_sessions/auth_account.yaml"
        ).await?;

        let mut auth = get_test_key(client, vec![Capability::ShareFiles]);

        let req = DownloadAuthorizationRequestBuilder::new()
            .for_bucket_id("8d625eb63be2775577c70e1a")
            .with_file_name_prefix("files/")
            .with_duration(chrono::Duration::seconds(30))?
            .build()?;

        let download_auth = get_download_authorization(&mut auth, req).await?;
        assert_eq!(download_auth.bucket_id(), "8d625eb63be2775577c70e1a");

        Ok(())
    }
}
