//! Typed domain and error contracts for Ronnie.

/// Generation-safe model acquisition.
pub mod acquire;
/// Typed v1 catalog clients.
pub mod api;
/// Bounded WAV decoding into planar audio.
pub mod audio;
/// Per-block byte caches and content/preparation identity.
pub mod cache;
/// Plain-data chain and session snapshots.
pub mod chain;
/// Bounded model and image downloads.
pub mod download;
/// Typed document and processing errors.
pub mod error;
mod fs;
/// Bounded HTTP transports and origin policy.
pub mod http;
/// Bounded JUCE variant and ValueTree codecs.
pub mod juce;
/// Bounded local discovery and tone loading.
pub mod local;
/// Legacy parameter-ID migration.
pub mod migration;
/// NAM document types.
pub mod model;
/// OAuth browser/device flows and PKCE.
pub mod oauth;
/// The OPTIMIZE switch and its pitch policy.
pub mod optimize;
/// Stable host parameter identities and persistence scopes.
pub mod params;
/// Bounded NAM JSON parsing.
pub mod parse;
/// Preset framing, filenames, and atomic writes.
pub mod preset_file;
/// Ordered file-backed preset management.
pub mod presets;
/// Reference-compatible XML properties.
pub mod properties;
/// Session refresh and offline account state.
pub mod session;
/// Atomic cross-process settings.
pub mod settings;
/// Content-addressed local model storage.
pub mod stash;
/// Session-state codec and offline restoration.
pub mod state;
/// Typed tones, model identity, and immutable model bytes.
pub mod tone;
/// Named instrument tunings for the whole-signal pitch shift.
pub mod tuning;

pub use acquire::{
    AcquiredModel, AcquisitionCoordinator, AcquisitionRequest, FetchRequest, ModelFetcher,
    SecretString,
};
pub use api::{
    ApiConfig, CatalogClient, Page, Profile, Taxonomy, TaxonomyEntry, TokenProvider, ToneQuery,
    ToneSort,
};
pub use audio::{AudioDocument, decode_wav};
pub use cache::{
    ByteCacheKey, ContentId, ContentPreparationKey, InstructionSet, ModelByteCache, ModelNamespace,
    PreparationKey, ReferencedModelBytes,
};
pub use chain::{
    ActivePreset, BlockId, BlockParams, ChainBlock, ChainBranch, ChainSide, ChainSnapshot, EqBand,
    EqBandType, EqParams, InputMode, MidiMapping, MidiSource, SessionSnapshot, model_namespace,
    references_model,
};
pub use download::{DecodedImage, DownloadClient};
pub use error::{
    AcquireError, ApiError, AudioError, CacheError, ChainError, CodecError, DownloadError,
    DspError, DspResult, FetchError, HttpError, LocalError, NamError, NamResult, OAuthError,
    PresetError, PropertiesError, RestoreError, SessionError, StashError, StateError, ToneError,
};
pub use http::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, OriginPolicy, RecordedTransport,
    ReqwestTransport, ResponseKind,
};
pub use juce::{JuceCodec, JuceTree, JuceValue};
pub use local::{
    LocalInput, LocalLoad, LocalModelInspection, discover_local, inspect_local_model,
    load_local_path, local_file_name_from_url,
};
pub use migration::{current_param_id, migrate_param_ids};
pub use model::{NamArchitecture, NamDocument, NamMetadata, VersionSupport};
pub use oauth::{
    CallbackOutcome, DeviceAuthorization, LoopbackCallback, LoopbackFlow, OAuthConfig, OAuthFlow,
    OAuthTokens, Pkce, parse_callback,
};
pub use optimize::{OPTIMIZE_PREFERENCE, PitchPlan, optimize_enabled, optimized_pitch};
pub use parse::parse_nam;
pub use preset_file::{
    PRESET_EXTENSION, PresetFraming, PresetHeader, read_preset, read_preset_file,
    read_preset_header, read_preset_header_file, sanitize_stem, unique_preset_path,
    write_preset_v2, write_preset_v2_file, write_user_preset,
};
pub use presets::{HostProgram, PresetInfo, PresetStore};
pub use properties::PropertiesFile;
pub use session::{SessionStore, TokenRefresher};
pub use settings::{PREFERENCES_SETTINGS, SettingsStore, UI_PREFERENCES_SETTINGS};
pub use stash::{LocalStash, StashedModel};
pub use state::{
    ModelCache, PreparationJob, RestoreCaches, RestorePlan, SessionDocument, StateCodec,
    restore_offline,
};
pub use tone::{ModelBytes, ModelId, ModelKind, ModelSource, Tone, ToneId, ToneModel};
pub use tuning::{Instrument, StringShift, Tuning};
