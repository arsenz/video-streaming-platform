use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct TranscodeJob {
    pub video_id: String,
    pub segment_name: String,
    pub resolution: String, // e.g., "480p"
}
// The payload we push to our SQS queue for the Segmentation Worker
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SegmentationJob {
    pub video_id: String,
    pub file_name: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct PlaylistJob {
    pub video_id: String,
    pub res: String,
    pub segment_count: u32,
}
