//! Media generation configuration (images, audio, video).
use serde::{Deserialize, Serialize};

fn default_image_model() -> String { "dall-e-3".to_string() }
fn default_image_size() -> String { "1024x1024".to_string() }
fn default_audio_model() -> String { "eleven_monolingual_v1".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaGenConfig {
    #[serde(default)] pub image_provider: Option<String>,
    #[serde(default)] pub image_api_key: Option<String>,
    #[serde(default = "default_image_model")] pub image_model: String,
    #[serde(default = "default_image_size")] pub image_size: String,
    #[serde(default)] pub audio_provider: Option<String>,
    #[serde(default)] pub audio_api_key: Option<String>,
    #[serde(default)] pub elevenlabs_voice_id: Option<String>,
    #[serde(default = "default_audio_model")] pub audio_model: String,
    #[serde(default)] pub video_provider: Option<String>,
    #[serde(default)] pub video_api_key: Option<String>,
}

impl Default for MediaGenConfig {
    fn default() -> Self {
        Self {
            image_provider: None, image_api_key: None,
            image_model: default_image_model(), image_size: default_image_size(),
            audio_provider: None, audio_api_key: None,
            elevenlabs_voice_id: None, audio_model: default_audio_model(),
            video_provider: None, video_api_key: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_gen_config_defaults() {
        let c = MediaGenConfig::default();
        assert!(c.image_provider.is_none());
        assert!(c.image_api_key.is_none());
        assert_eq!(c.image_model, "dall-e-3");
        assert_eq!(c.image_size, "1024x1024");
        assert!(c.audio_provider.is_none());
        assert!(c.video_provider.is_none());
    }
}
