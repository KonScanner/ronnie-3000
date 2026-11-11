//! Reference Default chrome and shared interaction controllers.

pub mod banners;
pub mod block;
pub mod block_info;
pub mod brand;
pub mod browser;
pub mod chain;
pub mod connection;
pub mod decks;
pub mod drag;
pub mod eq;
pub mod faceplate;
pub mod filters;
pub mod forms;
pub mod gear;
pub mod header;
pub mod hints;
pub mod key_setup;
pub mod lane;
pub mod midi_settings;
pub mod modals;
pub mod model_select;
pub mod overlays;
pub mod plugin_settings;
pub mod popover;
pub mod presets;
pub mod settings;
pub mod sign_in;
pub mod spectrum;
pub mod system_settings;
pub mod tile;
pub mod tile_menu;
pub mod toast;
pub mod tone_card;
pub mod tuner;
pub mod tuning;
pub mod update;

pub use self::decks::DeckKind;
pub use self::filters::FilterMenu;
pub use crate::services::{
    ART_SIDE, AVATAR_SIDE, AuthPhase, AuthService, AuthSnapshot,
    BrowserController as CatalogBrowserController, CatalogSeed, CatalogStatus, CatalogStore,
    ConnectionGate, ConnectionProblem, ImageCache, ImageFetcher, ImageRequest, LoadFlow, LoadPhase,
    LoadTarget, Session, SessionStatus, UpdateCheck, UpdateInfo,
};
pub use banners::{
    AudioDeviceState, BANNER_HEIGHT, BannerAction, BannerSpec, BannerStore, BannerVariant,
    BannerViewAction, WindowStep, banner_window_transition,
};
pub use block::{BlockDetail, BlockView};
pub use block_info::{BlockInfoPanel, InfoAction, InfoLayout, InfoPrompt};
pub use browser::{BrowserAction, BrowserController, BrowserView};
pub use chain::{ChainGallery, GallerySelection, TileVisualState};
pub use connection::ConnectionAction;
pub use decks::{DeckModel, MeasurementState};
pub use drag::DragOutcome;
pub use eq::{
    EqEditor, EqField, EqViewMode, format_freq, freq_to_norm, gain_to_y, has_gain, norm_to_freq,
    parse_decimal, parse_freq, reference_bands, slider_gain_at, type_default_q, type_label,
    y_to_gain,
};
pub use faceplate::Faceplate;
pub use forms::{FormRect, SnappedFormRect};
pub use header::{ChromeController, show};
pub use midi_settings::{MidiMapController, MidiMapSnapshot, MidiTarget, MidiTargetKind};
pub use modals::{BackdropUser, ModalCapture, ModalKind, ModalScrim, ModalStack};
pub use model_select::{ModelOption, ModelSelect};
pub use overlays::{DismissReason, Overlay};
pub use plugin_settings::PluginSettingsController;
pub use ronie_core::Profile;
pub use settings::{SettingsController, SettingsTab};
pub use sign_in::{SignInAction, SignInView};
pub use spectrum::SpectrumFeed;
pub use system_settings::{
    AudioDeviceOption, AudioInputChannel, MidiInputDevice, SystemDeviceState,
    SystemSettingsController,
};
pub use toast::ToastController;
pub use tone_card::ToneCardAction;
pub use tuner::{
    Pitch, TunerFeed, TunerState, TunerStatus, TunerView, lit_count, pitch_from_frequency,
};
pub use update::UpdateAction;
