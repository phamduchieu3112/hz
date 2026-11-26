//! Social Feed (Nhật ký) & Social Profile module for Zalo PC / Android
//! -------------------------------------------------------------------
//! Cung cấp cấu trúc dữ liệu và các API tương tác mạng xã hội thời gian thực:
//! - Lấy hồ sơ chi tiết bạn bè & chính mình (Ảnh bìa Cover HD, Avatar, Ngày sinh, Bio, lastActionTime)
//! - Lấy kho ảnh Nhật ký cá nhân (CDN zpsocial) & hợp nhất media chia sẻ thực tế
//! - Đóng gói binary RequestPacket chuẩn bytecode Zalo (`pw0.e`: cmd=1230/subcmd=6, `mw0.c`: cmd=2030/subcmd=0, `o41.g`: cmd=1234/2045)
//! - Nạp và hợp nhất bài viết Nhật ký thực tế + trạng thái like/comment thời gian thực từ SQLite (`timeline_feed_1`, `user_feed`, `table_feed_real_time_like_comment`)

use anyhow::{anyhow, Result};
use chrono::{FixedOffset, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeedAuthor {
    pub uid: String,
    pub display_name: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeedProfileFallback {
    pub uid: String,
    pub display_name: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedPostKind {
    #[default]
    Text,
    Photo,
    Album,
    Video,
    Link,
    SharedPost,
    AvatarUpdate,
    CoverUpdate,
    ProfileStatus,
}

impl FeedPostKind {
    pub fn badge_label(self) -> Option<&'static str> {
        match self {
            Self::Video => Some("Video"),
            Self::Album => Some("Album ảnh"),
            Self::Link => Some("Liên kết"),
            Self::SharedPost => Some("Chia sẻ bài viết"),
            Self::AvatarUpdate => Some("Ảnh đại diện"),
            Self::CoverUpdate => Some("Ảnh bìa"),
            Self::ProfileStatus => Some("Trạng thái"),
            Self::Text | Self::Photo => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeedLinkPreview {
    pub url: String,
    pub title: String,
    pub description: String,
    pub thumbnail: String,
    pub domain: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeedSharedPost {
    pub author_uid: String,
    pub author_name: String,
    pub content: String,
    pub link_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedMedia {
    pub photo_id: String,
    pub media_type: String, // "image" | "video"
    pub url: String,
    pub thumbnail: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedReaction {
    pub like_count: u32,
    pub comment_count: u32,
    pub is_liked: bool,
    pub top_reactions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedComment {
    pub comment_id: String,
    pub author_name: String,
    pub author_uid: String,
    pub content: String,
    pub timestamp_str: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    pub feed_id: String,
    pub author: FeedAuthor,
    pub date_str: String,
    pub content: String,
    pub status_badge: Option<String>,
    pub media_list: Vec<FeedMedia>,
    pub reaction: FeedReaction,
    pub comments: Vec<FeedComment>,
    #[serde(default)]
    pub timestamp_ms: i64,
    #[serde(default)]
    pub post_kind: FeedPostKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_preview: Option<FeedLinkPreview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_post: Option<FeedSharedPost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default)]
    pub is_profile_activity: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocialUserProfile {
    pub uid: String,
    pub username: String,
    pub display_name: String,
    pub zalo_name: String,
    pub avatar_url: String,
    pub cover_url: String,
    pub gender: i32,
    pub sdob: String,
    pub status: String,
    pub is_friend: bool,
    pub last_action_time: Option<u64>,
}

/// Strip Zalo profile-version suffixes like `"686454123_0"` so UIDs match across APIs.
pub fn normalize_feed_uid(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some((prefix, suffix)) = trimmed.split_once('_') {
        if !prefix.is_empty()
            && !suffix.is_empty()
            && prefix.bytes().all(|b| b.is_ascii_digit())
            && suffix.bytes().all(|b| b.is_ascii_digit())
        {
            return prefix.to_string();
        }
    }
    trimmed.to_string()
}

/// Returns `true` when `name` is empty, matches `uid`, or is just a raw numeric Zalo UID.
pub fn is_raw_uid_label(name: &str, uid: &str) -> bool {
    let t = name.trim();
    if t.is_empty() {
        return true;
    }
    let norm_uid = normalize_feed_uid(uid);
    let norm_t = normalize_feed_uid(t);
    if !uid.trim().is_empty() && (t == uid.trim() || (!norm_uid.is_empty() && norm_t == norm_uid))
    {
        return true;
    }
    norm_t.len() >= 6 && norm_t.bytes().all(|b| b.is_ascii_digit())
}

/// Filters out empty/placeholder/non-HTTP image URLs (e.g. Zalo `.../default`).
pub fn is_usable_social_image_url(url: &str) -> bool {
    let t = url.trim();
    if !(t.starts_with("http://") || t.starts_with("https://")) {
        return false;
    }
    let path_part = t.split(['?', '#']).next().unwrap_or(t).to_ascii_lowercase();
    !path_part.ends_with("/default") && !path_part.contains("/default.")
}

fn first_nonempty_str<'a>(candidates: &[Option<&'a Value>]) -> Option<&'a str> {
    candidates
        .iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
}

fn value_as_id_string(val: Option<&Value>) -> Option<String> {
    let v = val?;
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| v.as_i64().map(|n| n.to_string()))
        .or_else(|| v.as_u64().map(|n| n.to_string()))
}

fn normalize_timestamp_ms(raw_ts: i64) -> i64 {
    if raw_ts <= 0 {
        0
    } else if raw_ts < 100_000_000_000 {
        raw_ts.saturating_mul(1000)
    } else {
        raw_ts
    }
}

fn parse_i64_any(val: Option<&Value>) -> Option<i64> {
    let v = val?;
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

fn parse_u64_any(val: Option<&Value>) -> Option<u64> {
    let v = val?;
    v.as_u64()
        .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
}

pub fn format_vn_timestamp_ms(ts_ms: i64) -> String {
    if ts_ms <= 0 {
        return String::new();
    }
    let normalized_ms = normalize_timestamp_ms(ts_ms);
    let tz = FixedOffset::east_opt(7 * 3600).unwrap();
    tz.timestamp_millis_opt(normalized_ms)
        .single()
        .map(|dt| dt.format("%d/%m/%Y lúc %H:%M:%S").to_string())
        .unwrap_or_default()
}

pub fn normalize_social_media_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.ends_with(".jxl") && trimmed.len() > 4 {
        format!("{}.jpg", &trimmed[..trimmed.len() - 4])
    } else {
        trimmed.to_string()
    }
}

pub fn reaction_id_to_label(reaction_id: i64) -> &'static str {
    match reaction_id {
        1 => "Thích",
        2 => "Yêu thích",
        3 => "Haha",
        4 => "Wow",
        5 => "Buồn",
        6 => "Phẫn nộ",
        _ => "Thích",
    }
}

impl SocialUserProfile {
    pub fn from_json(uid: &str, val: &Value) -> Option<Self> {
        let obj = val.as_object()?;
        let fallback_uid = normalize_feed_uid(uid);
        let resolved_uid = value_as_id_string(obj.get("userId").or_else(|| obj.get("uid")))
            .map(|s| normalize_feed_uid(&s))
            .filter(|s| !s.is_empty())
            .unwrap_or(fallback_uid);

        let display_name = first_nonempty_str(&[
            obj.get("displayName"),
            obj.get("dpn"),
            obj.get("zaloName"),
            obj.get("zalo_name"),
            obj.get("name"),
            obj.get("username"),
        ])
        .unwrap_or(resolved_uid.as_str())
        .to_string();

        let zalo_name = obj
            .get("zaloName")
            .or_else(|| obj.get("zalo_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&display_name)
            .to_string();

        let username = obj
            .get("username")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();

        let avatar_url = [
            obj.get("avatar"),
            obj.get("avt"),
            obj.get("avatarUrl"),
            obj.get("avatar_url"),
        ]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|s| is_usable_social_image_url(s))
        .unwrap_or("")
        .to_string();

        let cover_url = obj
            .get("cover")
            .or_else(|| obj.get("bgavatar"))
            .and_then(Value::as_str)
            .or_else(|| {
                obj.get("cover")
                    .and_then(|c| c.get("coverUrl").or_else(|| c.get("url")))
                    .and_then(Value::as_str)
            })
            .map(str::trim)
            .filter(|s| is_usable_social_image_url(s))
            .unwrap_or("")
            .to_string();

        let gender = obj.get("gender").and_then(Value::as_i64).unwrap_or(0) as i32;
        let sdob = obj
            .get("sdob")
            .or_else(|| obj.get("dob"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let status = obj
            .get("status")
            .or_else(|| obj.get("statusMsg"))
            .or_else(|| obj.get("bio"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let is_friend = obj
            .get("isFr")
            .and_then(Value::as_i64)
            .map(|v| v == 1)
            .unwrap_or(true);
        let last_action_time = parse_u64_any(obj.get("lastActionTime"))
            .or_else(|| parse_u64_any(obj.get("lastUpdateTime")))
            .or_else(|| parse_u64_any(obj.get("lastUpdate")));

        Some(Self {
            uid: resolved_uid,
            username,
            display_name,
            zalo_name,
            avatar_url,
            cover_url,
            gender,
            sdob,
            status,
            is_friend,
            last_action_time,
        })
    }
}

/// Đóng gói binary `RequestPacket` chuẩn giao thức Zalo Social Feed (reverse từ `pw0.e`, `mw0.c`, `o41.g`, `nu0.v`)
pub mod packet {
    /// `pw0.e.d`: Get Timeline (`cmd = 1230` / `0x04CE`, `subcmd = 6`, `cmd_type = 3`)
    pub const CMD_FEED_TIMELINE: u16 = 1230;
    pub const SUBCMD_GET_FEED: u8 = 6;

    /// `mw0.c.k`: Get Friend User Timeline (`cmd = 2030` / `0x07EE`, `subcmd = 0`, `cmd_type = 3`)
    pub const CMD_USER_FEED: u16 = 2030;
    pub const SUBCMD_USER_FEED: u8 = 0;

    /// `o41.g`: Post Feed (`cmd = 1234` / `0x04D2`, `subcmd = 8`) & Edit Feed (`cmd = 2045` / `0x07FD`, `subcmd = 0`)
    pub const CMD_POST_FEED: u16 = 1234;
    pub const SUBCMD_POST_FEED: u8 = 8;
    pub const CMD_EDIT_FEED: u16 = 2045;
    pub const SUBCMD_EDIT_FEED: u8 = 0;

    /// Mã lỗi client trả về khi mất mạng (`!b2.M(false)`) trong `pw0.e`, `mw0.c`, `o41.g`
    pub const ERR_FEED_NO_NETWORK: i32 = 50001;

    /// Phiên bản client Zalo (`AbstractC27691d.f126612a` trong APK 26.10.02)
    pub const CLIENT_VERSION: i32 = 26_10_02_903;
    pub const CMD_TYPE_REQUEST: u8 = 3;

    /// Tạo payload nhị phân chuẩn `pw0.e.d` (52 bytes):
    /// `u8(1) + i32(client_version) + i32(lang_platform) + u8(tab) + i32(page) + i32(count)`
    /// `+ i64(last_feed_id_server) + i64(last_feed_id_local) + i64(max_feed_id) + i64(feed_version) + i16(source_enum)`
    pub fn build_timeline_payload_with_cursors(
        tab: u8,
        page: i32,
        count: i32,
        last_feed_id_server: i64,
        last_feed_id_local: i64,
        max_feed_id: i64,
        feed_version: i64,
        source_enum: i16,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(52);
        buf.push(0x01);
        buf.extend_from_slice(&CLIENT_VERSION.to_be_bytes());
        buf.extend_from_slice(&1i32.to_be_bytes()); // AbstractC21220r.l() = 1 (vi/android)
        buf.push(tab);
        buf.extend_from_slice(&page.to_be_bytes());
        buf.extend_from_slice(&count.to_be_bytes());
        buf.extend_from_slice(&last_feed_id_server.to_be_bytes());
        buf.extend_from_slice(&last_feed_id_local.to_be_bytes());
        buf.extend_from_slice(&max_feed_id.to_be_bytes());
        buf.extend_from_slice(&feed_version.to_be_bytes());
        buf.extend_from_slice(&source_enum.to_be_bytes());
        buf
    }

    /// Tạo payload nhị phân chuẩn `mw0.c.k` (18 bytes) khi xem dòng thời gian của 1 người dùng cụ thể (`cmd = 2030`)
    pub fn build_user_timeline_payload(owner_id: u32, flag: u8) -> Vec<u8> {
        let mut buf = Vec::with_capacity(18);
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.push(0x01);
        buf.extend_from_slice(&CLIENT_VERSION.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&(owner_id as i32).to_be_bytes());
        buf.extend_from_slice(&1i32.to_be_bytes());
        buf.push(flag);
        buf
    }

    /// Tạo payload cho lệnh GET_FEED (tương thích chữ ký caller `modals.rs`, xuất đúng 49 bytes theo `pw0.e.d`)
    pub fn build_get_feed_payload(
        owner_id: u64,
        viewer_id: u64,
        tab: u32,
        page: u32,
        count: u32,
    ) -> Vec<u8> {
        let _ = viewer_id;
        build_timeline_payload_with_cursors(
            tab as u8,
            page as i32,
            count as i32,
            0,
            0,
            owner_id as i64,
            2, // C26856y.y() feed_version = 2
            0,
        )
    }

    /// Đóng gói payload vào khung `RequestPacket` chuẩn (`nu0.v`):
    /// `[total_len: u32 BE][version=1: u8][encrypt=0: u8][uin: u32 BE][cmd_type=3: u8][cmd: u16 BE][subcmd: u8][seq_id: u16 BE][params_len: u32 BE][payload]`
    pub fn build_request_packet_with_uin(
        uin: u32,
        cmd: u16,
        subcmd: u8,
        seq_id: u16,
        payload: &[u8],
    ) -> Vec<u8> {
        let header_len = 1 + 1 + 4 + 1 + 2 + 1 + 2 + 4; // 16 bytes
        let body_len = header_len + payload.len();
        let mut packet = Vec::with_capacity(4 + body_len);
        packet.extend_from_slice(&(body_len as u32).to_be_bytes());
        packet.push(0x01); // version = 1 (n(1))
        packet.push(0x00); // encrypt = 0 (g(0))
        packet.extend_from_slice(&uin.to_be_bytes()); // m(uin)
        packet.push(CMD_TYPE_REQUEST); // f((byte) 3)
        packet.extend_from_slice(&cmd.to_be_bytes()); // d((short) cmd)
        packet.push(subcmd); // l((byte) subcmd)
        packet.extend_from_slice(&seq_id.to_be_bytes()); // k(seqId)
        packet.extend_from_slice(&(payload.len() as u32).to_be_bytes()); // i(payload.len)
        packet.extend_from_slice(payload);
        packet
    }

    /// Đóng gói `RequestPacket` mặc định (tương thích chữ ký caller `modals.rs`)
    pub fn build_request_packet(cmd: u16, subcmd: u8, payload: &[u8]) -> Vec<u8> {
        build_request_packet_with_uin(0, cmd, subcmd, 1, payload)
    }
}

// Mở rộng các phương thức mạng xã hội cho Session
impl crate::session::Session {
    /// Lấy hồ sơ mạng xã hội của chính mình (`/api/social/profile/me-v2` hoặc fallback `getprofiles`)
    pub async fn get_my_social_profile(&self) -> Result<SocialUserProfile> {
        let my_uid = self
            .login
            .as_ref()
            .map(|l| l.uin.clone())
            .unwrap_or_default();
        if my_uid.is_empty() {
            return Err(anyhow!("Missing logged-in UIN"));
        }

        let body = json!({
            "avatar_size": 240,
            "imei": self.imei,
        });
        if let Ok(enc) = self.encoded_params(&body) {
            let url = format!(
                "{}/api/social/profile/me-v2?{}&params={enc}&os=8",
                self.domains.profile,
                self.common_params()
            );
            if let Ok(resp) = self.get(&url).await {
                let dec = self.maybe_decrypt(resp, self.session_key_bytes()).await;
                if dec.get("error_code").and_then(Value::as_i64) == Some(0) {
                    if let Some(data) = dec.get("data") {
                        let target_obj = data.get(&my_uid).unwrap_or(data);
                        if let Some(p) = SocialUserProfile::from_json(&my_uid, target_obj) {
                            return Ok(p);
                        }
                    }
                }
            }
        }

        self.get_friend_social_profile(&my_uid).await
    }

    /// Lấy hồ sơ chi tiết bạn bè (Ảnh bìa Cover HD, Avatar, Ngày sinh, Giới tính, Bio)
    pub async fn get_friend_social_profile(&self, target_uid: &str) -> Result<SocialUserProfile> {
        let body = json!({
            "avatar_size": 240,
            "friend_ids": [target_uid],
            "imei": self.imei,
        });

        let enc = self.encoded_params(&body)?;
        let url = format!(
            "{}/api/social/friend/getprofiles?{}&params={enc}&os=8",
            self.domains.profile,
            self.common_params()
        );

        let resp = self.get(&url).await?;
        let dec = self.maybe_decrypt(resp, self.session_key_bytes()).await;

        if dec.get("error_code").and_then(Value::as_i64) == Some(0) {
            if let Some(prof_obj) = dec.get("data").and_then(|d| d.get(target_uid).or(Some(d))) {
                if let Some(p) = SocialUserProfile::from_json(target_uid, prof_obj) {
                    return Ok(p);
                }
            }
        }

        // Fallback sang batch v2 cho 1 UID nếu endpoint v1 trả lỗi
        if let Ok(mut list) = self
            .get_friend_social_profiles_batch(&[target_uid.to_string()])
            .await
        {
            if let Some(p) = list.pop() {
                return Ok(p);
            }
        }

        Err(anyhow!(
            "Server returned error or missing data for uid {target_uid}: {dec}"
        ))
    }

    /// Lấy kho ảnh Nhật ký cá nhân (CDN zpsocial)
    pub async fn get_my_social_avatar_list(&self, page: u32, count: u32) -> Result<Vec<FeedMedia>> {
        let body = json!({
            "page": page,
            "count": count,
            "imei": self.imei,
        });

        let enc = self.encoded_params(&body)?;
        let url = format!(
            "{}/api/social/avatar-list?{}&params={enc}&os=8",
            self.domains.profile,
            self.common_params()
        );

        let resp = self.get(&url).await?;
        let dec = self.maybe_decrypt(resp, self.session_key_bytes()).await;

        let mut photos = Vec::new();
        if let Some(photo_arr) = dec
            .get("data")
            .and_then(|d| d.get("photos"))
            .and_then(Value::as_array)
        {
            for p in photo_arr {
                let photo_id = p
                    .get("photoId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let raw_url = p.get("url").and_then(Value::as_str).unwrap_or("");
                let url = normalize_social_media_url(raw_url);
                let thumbnail = p
                    .get("thumbnail")
                    .and_then(Value::as_str)
                    .map(normalize_social_media_url)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| url.clone());
                let width = p.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                let height = p.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                if !url.is_empty() {
                    photos.push(FeedMedia {
                        photo_id,
                        media_type: "image".to_string(),
                        url,
                        thumbnail,
                        width,
                        height,
                        duration_secs: None,
                    });
                }
            }
        }

        Ok(photos)
    }

    /// Lấy hàng loạt hồ sơ mạng xã hội của nhiều bạn bè (`POST /api/social/friend/getprofiles/v2`)
    pub async fn get_friend_social_profiles_batch(
        &self,
        target_uids: &[String],
    ) -> Result<Vec<SocialUserProfile>> {
        let mut normalized_uids = Vec::new();
        let mut input_seen = HashSet::new();
        for raw in target_uids {
            let norm = normalize_feed_uid(raw);
            if !norm.is_empty() && input_seen.insert(norm.clone()) {
                normalized_uids.push(norm);
            }
        }
        if normalized_uids.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_profiles = Vec::new();
        let mut seen_uids = HashSet::new();

        for chunk in normalized_uids.chunks(40) {
            let pversion_map: Vec<String> = chunk.iter().map(|u| format!("{u}_0")).collect();
            let body = json!({
                "phonebook_version": 0,
                "friend_pversion_map": pversion_map,
                "avatar_size": 240,
                "language": "vi",
                "show_online_status": 1,
                "imei": self.imei,
            });
            let Ok(enc) = self.encoded_params(&body) else {
                continue;
            };
            let url = format!(
                "{}/api/social/friend/getprofiles/v2?{}",
                self.domains.profile,
                self.common_params()
            );
            let form = format!("params={enc}");
            let Ok(resp) = self.post_form(&url, form).await else {
                continue;
            };
            let dec = self.maybe_decrypt(resp, self.session_key_bytes()).await;

            if let Some(changed) = dec
                .get("data")
                .and_then(|d| d.get("changed_profiles").or(Some(d)))
                .and_then(Value::as_object)
            {
                for (raw_key, pval) in changed {
                    let norm_key = normalize_feed_uid(raw_key);
                    if let Some(p) = SocialUserProfile::from_json(&norm_key, pval) {
                        let dedup_uid = normalize_feed_uid(&p.uid);
                        if !dedup_uid.is_empty() && seen_uids.insert(dedup_uid) {
                            all_profiles.push(p);
                        }
                    }
                }
            }
        }

        Ok(all_profiles)
    }

    /// Tổng hợp Dòng thời gian (Nhật ký) từ:
    /// 1. Hồ sơ cá nhân & kho ảnh đại diện/nhật ký (`/api/social/avatar-list`, `/api/social/profile/me-v2`)
    /// 2. Hồ sơ mạng xã hội thời gian thực của toàn bộ bạn bè (`/api/social/friend/getprofiles/v2`) + fallback từ danh bạ/hội thoại
    /// 3. Cơ sở dữ liệu Nhật ký thực tế (`timeline_feed_1`, `user_feed`, `table_feed_real_time_like_comment`)
    /// 4. Thẻ cập nhật trạng thái/ảnh bìa thực tế từ API hồ sơ (không bịa bài đăng từ ảnh chat 1-1)
    pub async fn fetch_timeline_feed(
        &self,
        friend_uids: &[String],
        known_contacts: &HashMap<String, FeedProfileFallback>,
    ) -> Result<(Vec<SocialUserProfile>, Vec<FeedItem>)> {
        let mut items: Vec<FeedItem> = Vec::new();
        let mut seen_feed_ids: HashSet<String> = HashSet::new();

        // Chuẩn hóa danh bạ fallback theo UID
        let mut norm_contacts: HashMap<String, FeedProfileFallback> = HashMap::new();
        for (raw_uid, fb) in known_contacts {
            let uid = normalize_feed_uid(if fb.uid.trim().is_empty() {
                raw_uid
            } else {
                &fb.uid
            });
            if uid.is_empty() {
                continue;
            }
            norm_contacts
                .entry(uid.clone())
                .and_modify(|existing| {
                    if is_raw_uid_label(&existing.display_name, &existing.uid)
                        && !is_raw_uid_label(&fb.display_name, &uid)
                    {
                        existing.display_name = fb.display_name.trim().to_string();
                    }
                    if !is_usable_social_image_url(&existing.avatar_url)
                        && is_usable_social_image_url(&fb.avatar_url)
                    {
                        existing.avatar_url = fb.avatar_url.trim().to_string();
                    }
                })
                .or_insert_with(|| FeedProfileFallback {
                    uid,
                    display_name: fb.display_name.trim().to_string(),
                    avatar_url: if is_usable_social_image_url(&fb.avatar_url) {
                        fb.avatar_url.trim().to_string()
                    } else {
                        String::new()
                    },
                });
        }

        // 1. Lấy hồ sơ cá nhân & kho ảnh đại diện/nhật ký của chính mình
        let my_uid = normalize_feed_uid(
            self.login
                .as_ref()
                .map(|l| l.uin.as_str())
                .unwrap_or_default(),
        );
        let mut my_name = self
            .login
            .as_ref()
            .map(|l| l.display_name.clone())
            .unwrap_or_else(|| "Tôi".to_string());
        let mut my_ava = self
            .login
            .as_ref()
            .map(|l| l.avatar.clone())
            .filter(|u| is_usable_social_image_url(u))
            .unwrap_or_default();

        let my_profile_opt = self.get_my_social_profile().await.ok();
        if let Some(ref mp) = my_profile_opt {
            if !is_raw_uid_label(&mp.display_name, &mp.uid) {
                my_name = mp.display_name.clone();
            }
            if is_usable_social_image_url(&mp.avatar_url) {
                my_ava = mp.avatar_url.clone();
            }
        }
        if is_raw_uid_label(&my_name, &my_uid) {
            if let Some(fb) = norm_contacts.get(&my_uid) {
                if !is_raw_uid_label(&fb.display_name, &my_uid) {
                    my_name = fb.display_name.clone();
                }
                if !is_usable_social_image_url(&my_ava)
                    && is_usable_social_image_url(&fb.avatar_url)
                {
                    my_ava = fb.avatar_url.clone();
                }
            }
        }

        if let Ok(my_photos) = self.get_my_social_avatar_list(1, 20).await {
            if !my_photos.is_empty() {
                let my_ts_ms = my_profile_opt
                    .as_ref()
                    .and_then(|p| p.last_action_time)
                    .map(|t| normalize_timestamp_ms(t as i64))
                    .unwrap_or(0);
                let feed_id = format!("my_album_{my_uid}");
                seen_feed_ids.insert(feed_id.clone());
                let post_kind = FeedPostKind::AvatarUpdate;
                let mut author = FeedAuthor {
                    uid: my_uid.clone(),
                    display_name: my_name.clone(),
                    avatar_url: my_ava.clone(),
                };
                if is_raw_uid_label(&author.display_name, &author.uid) {
                    author.display_name = "Tôi".to_string();
                }
                items.push(FeedItem {
                    feed_id,
                    author,
                    date_str: if my_ts_ms > 0 {
                        format_vn_timestamp_ms(my_ts_ms)
                    } else {
                        "Kho ảnh cá nhân".to_string()
                    },
                    content: my_profile_opt
                        .as_ref()
                        .filter(|p| !p.status.trim().is_empty())
                        .map(|p| p.status.clone())
                        .unwrap_or_else(|| {
                            format!("Album ảnh đại diện mạng xã hội ({} ảnh)", my_photos.len())
                        }),
                    status_badge: post_kind.badge_label().map(str::to_string),
                    media_list: my_photos,
                    reaction: FeedReaction {
                        like_count: 0,
                        comment_count: 0,
                        is_liked: false,
                        top_reactions: Vec::new(),
                    },
                    comments: Vec::new(),
                    timestamp_ms: my_ts_ms,
                    post_kind,
                    link_preview: None,
                    shared_post: None,
                    location: None,
                    is_profile_activity: true,
                });
            }
        }

        // 2. Lấy hàng loạt hồ sơ mạng xã hội của bạn bè theo thời gian thực (không giới hạn 120)
        let mut ordered_uids: Vec<String> = Vec::new();
        let mut uid_set: HashSet<String> = HashSet::new();
        for u in friend_uids {
            let norm = normalize_feed_uid(u);
            if !norm.is_empty() && norm != my_uid && uid_set.insert(norm.clone()) {
                ordered_uids.push(norm);
            }
        }
        for u in norm_contacts.keys() {
            if !u.is_empty() && *u != my_uid && uid_set.insert(u.clone()) {
                ordered_uids.push(u.clone());
            }
        }

        let batch_ids: Vec<String> = ordered_uids.into_iter().take(1000).collect();
        let mut profiles = self
            .get_friend_social_profiles_batch(&batch_ids)
            .await
            .unwrap_or_default();

        // Fallback: nếu getprofiles/v2 trả rỗng thì gọi get_friend_social_profile cho 10 bạn đầu
        if profiles.is_empty() {
            for uid in batch_ids.iter().take(10) {
                if let Ok(p) = self.get_friend_social_profile(uid).await {
                    profiles.push(p);
                }
            }
        }

        if let Some(mp) = my_profile_opt {
            let mp_uid = normalize_feed_uid(&mp.uid);
            if !profiles.iter().any(|p| normalize_feed_uid(&p.uid) == mp_uid) {
                profiles.push(mp);
            }
        }

        // Bù tên hiển thị và avatar từ danh bạ/hội thoại cho các hồ sơ thiếu hoặc còn dạng UID số
        let mut present_uids: HashSet<String> = HashSet::new();
        for p in &mut profiles {
            p.uid = normalize_feed_uid(&p.uid);
            present_uids.insert(p.uid.clone());
            if let Some(fb) = norm_contacts.get(&p.uid) {
                if is_raw_uid_label(&p.display_name, &p.uid)
                    && !is_raw_uid_label(&fb.display_name, &p.uid)
                {
                    p.display_name = fb.display_name.clone();
                }
                if !is_usable_social_image_url(&p.avatar_url)
                    && is_usable_social_image_url(&fb.avatar_url)
                {
                    p.avatar_url = fb.avatar_url.clone();
                }
            }
        }
        for (uid, fb) in &norm_contacts {
            if *uid != my_uid
                && !present_uids.contains(uid)
                && !is_raw_uid_label(&fb.display_name, uid)
            {
                present_uids.insert(uid.clone());
                profiles.push(SocialUserProfile {
                    uid: uid.clone(),
                    username: String::new(),
                    display_name: fb.display_name.clone(),
                    zalo_name: fb.display_name.clone(),
                    avatar_url: fb.avatar_url.clone(),
                    cover_url: String::new(),
                    gender: 0,
                    sdob: String::new(),
                    status: String::new(),
                    is_friend: true,
                    last_action_time: None,
                });
            }
        }

        // 3. Nạp bài đăng Nhật ký thực tế từ cơ sở dữ liệu SQLite qua spawn_blocking
        let mut db_candidates = Vec::new();
        if let Ok(env_db) = std::env::var("ZALO_TIMELINE_DB") {
            if !env_db.trim().is_empty() {
                db_candidates.push(env_db);
            }
        }
        if crate::account_manager::is_safe_uin(&my_uid) {
            db_candidates.push(format!("accounts/{my_uid}_history.db"));
        }
        db_candidates.push("zalo_history.db".to_string());
        db_candidates.push("databases/zalo".to_string());
        db_candidates.push("test_lab/databases/zalo".to_string());

        let loaded_sqlite_feeds: Vec<FeedItem> = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            let mut seen_paths = HashSet::new();
            for db_path in db_candidates {
                if !seen_paths.insert(db_path.clone()) {
                    continue;
                }
                if std::path::Path::new(&db_path).exists() {
                    if let Ok(real_feeds) = load_real_timeline_from_sqlite(&db_path, "") {
                        out.extend(real_feeds);
                    }
                }
            }
            out
        })
        .await
        .unwrap_or_default();

        // Nếu bài viết hoặc bình luận từ SQLite có UID chưa có trong profiles/norm_contacts, gọi batch bổ sung
        let mut missing_uids = Vec::new();
        let mut missing_seen = HashSet::new();
        for rf in &loaded_sqlite_feeds {
            for candidate in std::iter::once(rf.author.uid.as_str())
                .chain(rf.comments.iter().map(|c| c.author_uid.as_str()))
            {
                let norm = normalize_feed_uid(candidate);
                if !norm.is_empty()
                    && !present_uids.contains(&norm)
                    && !norm_contacts.contains_key(&norm)
                    && missing_seen.insert(norm.clone())
                {
                    missing_uids.push(norm);
                }
            }
        }
        if !missing_uids.is_empty() {
            if let Ok(extra) = self.get_friend_social_profiles_batch(&missing_uids).await {
                for mut p in extra {
                    p.uid = normalize_feed_uid(&p.uid);
                    if !p.uid.is_empty() && present_uids.insert(p.uid.clone()) {
                        profiles.push(p);
                    }
                }
            }
        }

        // Sắp xếp hồ sơ theo thời gian hoạt động gần nhất (real-time)
        profiles.sort_by(|a, b| {
            b.last_action_time
                .unwrap_or(0)
                .cmp(&a.last_action_time.unwrap_or(0))
                .then_with(|| a.display_name.cmp(&b.display_name))
        });

        for mut rf in loaded_sqlite_feeds {
            if !seen_feed_ids.insert(rf.feed_id.clone()) {
                continue;
            }
            resolve_feed_author(&mut rf.author, &profiles, &norm_contacts);
            for c in &mut rf.comments {
                let mut c_author = FeedAuthor {
                    uid: c.author_uid.clone(),
                    display_name: c.author_name.clone(),
                    avatar_url: String::new(),
                };
                resolve_feed_author(&mut c_author, &profiles, &norm_contacts);
                c.author_uid = c_author.uid;
                c.author_name = c_author.display_name;
            }
            items.push(rf);
        }

        // 4. Với bạn bè có trạng thái (status) hoặc ảnh bìa (cover_url) thực tế từ API hồ sơ
        // mà chưa có bài đăng nào trong danh sách, hiển thị thẻ hoạt động hồ sơ thực tế của họ
        for prof in &profiles {
            if prof.uid == my_uid {
                continue;
            }
            let has_status = !prof.status.trim().is_empty();
            let has_cover = is_usable_social_image_url(&prof.cover_url);
            if !has_status && !has_cover {
                continue;
            }
            if items.iter().any(|it| it.author.uid == prof.uid) {
                continue;
            }

            let feed_id = format!("profile_activity_{}", prof.uid);
            if !seen_feed_ids.insert(feed_id.clone()) {
                continue;
            }

            let ts_ms = prof
                .last_action_time
                .map(|t| normalize_timestamp_ms(t as i64))
                .unwrap_or(0);
            let date_str = if ts_ms > 0 {
                format_vn_timestamp_ms(ts_ms)
            } else {
                "Hoạt động hồ sơ".to_string()
            };

            let mut media_list = Vec::new();
            if has_cover {
                let norm_cover = normalize_social_media_url(&prof.cover_url);
                media_list.push(FeedMedia {
                    photo_id: format!("cover_{}", prof.uid),
                    media_type: "image".to_string(),
                    url: norm_cover.clone(),
                    thumbnail: norm_cover,
                    width: 0,
                    height: 0,
                    duration_secs: None,
                });
            }

            let post_kind = if has_cover {
                FeedPostKind::CoverUpdate
            } else {
                FeedPostKind::ProfileStatus
            };
            let badge = if has_status && has_cover {
                Some("Tiểu sử & Ảnh bìa".to_string())
            } else {
                post_kind.badge_label().map(str::to_string)
            };

            let mut author = FeedAuthor {
                uid: prof.uid.clone(),
                display_name: prof.display_name.clone(),
                avatar_url: prof.avatar_url.clone(),
            };
            resolve_feed_author(&mut author, &profiles, &norm_contacts);

            items.push(FeedItem {
                feed_id,
                author,
                date_str,
                content: prof.status.clone(),
                status_badge: badge,
                media_list,
                reaction: FeedReaction {
                    like_count: 0,
                    comment_count: 0,
                    is_liked: false,
                    top_reactions: Vec::new(),
                },
                comments: Vec::new(),
                timestamp_ms: ts_ms,
                post_kind,
                link_preview: None,
                shared_post: None,
                location: None,
                is_profile_activity: true,
            });
        }

        // Sắp xếp toàn bộ dòng thời gian theo timestamp thực tế mới nhất lên đầu;
        // bài đăng Nhật ký thực tế ưu tiên đứng trên thẻ hoạt động hồ sơ cùng mốc/chưa có mốc thời gian
        items.sort_by(|a, b| {
            b.timestamp_ms
                .cmp(&a.timestamp_ms)
                .then_with(|| a.is_profile_activity.cmp(&b.is_profile_activity))
                .then_with(|| b.feed_id.cmp(&a.feed_id))
        });

        Ok((profiles, items))
    }
}

/// Resolve an item's author display name & avatar strictly by normalized UID first.
/// Never overwrites a non-empty UID by matching a display name.
pub fn resolve_feed_author(
    author: &mut FeedAuthor,
    profiles: &[SocialUserProfile],
    known_contacts: &HashMap<String, FeedProfileFallback>,
) {
    author.uid = normalize_feed_uid(&author.uid);
    if !is_usable_social_image_url(&author.avatar_url) {
        author.avatar_url.clear();
    }

    if !author.uid.is_empty() {
        if let Some(prof) = profiles
            .iter()
            .find(|p| normalize_feed_uid(&p.uid) == author.uid)
        {
            if is_raw_uid_label(&author.display_name, &author.uid)
                && !is_raw_uid_label(&prof.display_name, &author.uid)
            {
                author.display_name = prof.display_name.trim().to_string();
            }
            if author.avatar_url.is_empty() && is_usable_social_image_url(&prof.avatar_url) {
                author.avatar_url = prof.avatar_url.trim().to_string();
            }
        }
        if let Some(fb) = known_contacts.get(&author.uid) {
            if is_raw_uid_label(&author.display_name, &author.uid)
                && !is_raw_uid_label(&fb.display_name, &author.uid)
            {
                author.display_name = fb.display_name.trim().to_string();
            }
            if author.avatar_url.is_empty() && is_usable_social_image_url(&fb.avatar_url) {
                author.avatar_url = fb.avatar_url.trim().to_string();
            }
        }
    } else if !author.display_name.trim().is_empty() {
        let needle = author.display_name.trim();
        let mut matched_uid: Option<String> = None;
        let mut matched_name: Option<String> = None;
        let mut matched_ava: Option<String> = None;
        let mut ambiguous = false;

        for p in profiles {
            let matches = p.display_name.trim().eq_ignore_ascii_case(needle)
                || p.zalo_name.trim().eq_ignore_ascii_case(needle);
            if matches {
                let norm = normalize_feed_uid(&p.uid);
                if let Some(ref prev) = matched_uid {
                    if prev != &norm {
                        ambiguous = true;
                        break;
                    }
                } else {
                    matched_uid = Some(norm);
                    matched_name = Some(p.display_name.trim().to_string());
                    if is_usable_social_image_url(&p.avatar_url) {
                        matched_ava = Some(p.avatar_url.trim().to_string());
                    }
                }
            }
        }
        if !ambiguous {
            for (uid, fb) in known_contacts {
                if fb.display_name.trim().eq_ignore_ascii_case(needle) {
                    let norm = normalize_feed_uid(uid);
                    if let Some(ref prev) = matched_uid {
                        if prev != &norm {
                            ambiguous = true;
                            break;
                        }
                    } else {
                        matched_uid = Some(norm);
                        matched_name = Some(fb.display_name.trim().to_string());
                        if is_usable_social_image_url(&fb.avatar_url) {
                            matched_ava = Some(fb.avatar_url.trim().to_string());
                        }
                    }
                }
            }
        }
        if !ambiguous {
            if let Some(uid) = matched_uid {
                author.uid = uid;
                if let Some(n) = matched_name.filter(|s| !s.is_empty()) {
                    author.display_name = n;
                }
                if author.avatar_url.is_empty() {
                    if let Some(a) = matched_ava {
                        author.avatar_url = a;
                    }
                }
            }
        }
    }

    if is_raw_uid_label(&author.display_name, &author.uid) {
        author.display_name = "Người dùng Zalo".to_string();
    }
    if !is_usable_social_image_url(&author.avatar_url) {
        author.avatar_url.clear();
    }
}

/// Strict UID-based feed item filter (never matches a different UID by display name).
pub fn filter_feed_item_for_target(item: &FeedItem, target_uid: &str) -> bool {
    let target = normalize_feed_uid(target_uid);
    if target.is_empty() {
        return true;
    }
    let author = normalize_feed_uid(&item.author.uid);
    !author.is_empty() && author == target
}

#[derive(Debug, Clone, Default)]
struct RealtimeFeedStat {
    like_count: Option<u32>,
    comment_count: Option<u32>,
    is_liked: Option<bool>,
}

fn load_realtime_like_comment_map(conn: &rusqlite::Connection) -> HashMap<String, RealtimeFeedStat> {
    let mut map = HashMap::new();
    let exists: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='table_feed_real_time_like_comment'",
            [],
            |row| row.get(0),
        )
        .map(|count: i64| count > 0)
        .unwrap_or(false);

    if !exists {
        return map;
    }

    if let Ok(mut stmt) = conn.prepare(
        "SELECT fid, tot, is_like, tot_cmt FROM table_feed_real_time_like_comment",
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            let fid: String = row.get(0)?;
            let tot: i64 = row.get(1).unwrap_or(-1);
            let is_like: i64 = row.get(2).unwrap_or(-1);
            let tot_cmt: i64 = row.get(3).unwrap_or(-1);
            Ok((fid, tot, is_like, tot_cmt))
        }) {
            for r in rows.flatten() {
                let (fid, tot, is_like, tot_cmt) = r;
                if fid.is_empty() {
                    continue;
                }
                map.insert(
                    fid,
                    RealtimeFeedStat {
                        like_count: if tot >= 0 { Some(tot as u32) } else { None },
                        comment_count: if tot_cmt >= 0 {
                            Some(tot_cmt as u32)
                        } else {
                            None
                        },
                        is_liked: if is_like >= 0 {
                            Some(is_like > 0)
                        } else {
                            None
                        },
                    },
                );
            }
        }
    }

    map
}

fn extract_media_from_photo_val(photo: &Value, default_dim: Option<&Value>) -> Option<FeedMedia> {
    let photoid = value_as_id_string(
        photo
            .get("photoid")
            .or_else(|| photo.get("photoId"))
            .or_else(|| photo.get("id")),
    )
    .unwrap_or_default();

    let raw_origin = first_nonempty_str(&[
        photo.get("origin"),
        photo.get("url"),
        photo.get("hd"),
        photo.get("large"),
        photo.get("normal"),
        photo.get("full"),
        photo.get("src"),
        photo.get("href"),
        photo.get("thumb"),
    ])
    .unwrap_or("");

    let raw_video = first_nonempty_str(&[
        photo.get("videourl"),
        photo.get("videoUrl"),
        photo.get("video_url"),
        photo.get("streamingUrl"),
        photo.get("playUrl"),
        photo.get("mp4"),
    ])
    .unwrap_or("");

    let type_hint = photo
        .get("type")
        .or_else(|| photo.get("mediaType"))
        .or_else(|| photo.get("media_type"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();

    let (media_type, primary_url) = if !raw_video.is_empty() {
        ("video".to_string(), normalize_social_media_url(raw_video))
    } else if type_hint == "video" && !raw_origin.is_empty() {
        ("video".to_string(), normalize_social_media_url(raw_origin))
    } else if !raw_origin.is_empty() {
        ("image".to_string(), normalize_social_media_url(raw_origin))
    } else {
        return None;
    };

    let thumbnail = [
        photo.get("thumb"),
        photo.get("thumbnail"),
        photo.get("cover"),
        photo.get("poster"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(normalize_social_media_url)
    .find(|s| !s.is_empty())
    .unwrap_or_else(|| {
        if media_type == "image" {
            primary_url.clone()
        } else {
            String::new()
        }
    });

    let dim = photo
        .get("photo_dimension")
        .or_else(|| photo.get("dimension"))
        .or_else(|| photo.get("size"))
        .or(default_dim);
    let width = dim
        .and_then(|d| d.get("width").or_else(|| d.get("w")))
        .or_else(|| photo.get("width").or_else(|| photo.get("w")))
        .and_then(|v| parse_u64_any(Some(v)))
        .unwrap_or(0) as u32;
    let height = dim
        .and_then(|d| d.get("height").or_else(|| d.get("h")))
        .or_else(|| photo.get("height").or_else(|| photo.get("h")))
        .and_then(|v| parse_u64_any(Some(v)))
        .unwrap_or(0) as u32;

    let duration_secs = parse_u64_any(
        photo
            .get("duration")
            .or_else(|| photo.get("videoDuration"))
            .or_else(|| photo.get("durationSec")),
    )
    .map(|d| if d > 36_000 { (d / 1000) as u32 } else { d as u32 })
    .filter(|&d| d > 0);

    Some(FeedMedia {
        photo_id: photoid,
        media_type,
        url: primary_url,
        thumbnail,
        width,
        height,
        duration_secs,
    })
}

fn extract_link_preview(item: &Value, content_obj: &Value) -> Option<FeedLinkPreview> {
    let link_val = [
        content_obj.get("link"),
        content_obj.get("link_info"),
        content_obj.get("share_link"),
        item.get("link"),
        content_obj.get("attachment"),
    ]
    .into_iter()
    .flatten()
    .find(|v| v.is_object() || v.as_str().is_some_and(|s| s.starts_with("http")))?;

    if let Some(raw_url) = link_val.as_str() {
        let u = raw_url.trim();
        if u.is_empty() {
            return None;
        }
        return Some(FeedLinkPreview {
            url: u.to_string(),
            title: String::new(),
            description: String::new(),
            thumbnail: String::new(),
            domain: extract_url_host(u),
        });
    }

    let url = first_nonempty_str(&[
        link_val.get("url"),
        link_val.get("href"),
        link_val.get("link"),
    ])
    .unwrap_or("")
    .to_string();

    let title = first_nonempty_str(&[link_val.get("title"), link_val.get("name")])
        .unwrap_or("")
        .to_string();

    if url.is_empty() && title.is_empty() {
        return None;
    }

    let description = first_nonempty_str(&[link_val.get("desc"), link_val.get("description")])
        .unwrap_or("")
        .to_string();

    let thumbnail = [
        link_val.get("thumb"),
        link_val.get("thumbnail"),
        link_val.get("image"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(normalize_social_media_url)
    .find(|s| is_usable_social_image_url(s))
    .unwrap_or_default();

    let domain = first_nonempty_str(&[
        link_val.get("domain"),
        link_val.get("src"),
        link_val.get("host"),
    ])
    .map(str::to_string)
    .unwrap_or_else(|| extract_url_host(&url));

    Some(FeedLinkPreview {
        url,
        title,
        description,
        thumbnail,
        domain,
    })
}

fn extract_url_host(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim_start_matches("www.")
        .to_string()
}

fn extract_shared_post(item: &Value, content_obj: &Value) -> Option<FeedSharedPost> {
    let sp = [
        item.get("share_feed"),
        item.get("origin_feed"),
        item.get("shared_post"),
        content_obj.get("share_feed"),
        content_obj.get("origin_feed"),
    ]
    .into_iter()
    .flatten()
    .find(|v| v.is_object())?;

    let header = sp.get("header").or_else(|| sp.get("author"));
    let author_uid = value_as_id_string(
        header
            .and_then(|h| h.get("uid").or_else(|| h.get("userId")))
            .or_else(|| sp.get("uid").or_else(|| sp.get("authorUid"))),
    )
    .map(|s| normalize_feed_uid(&s))
    .unwrap_or_default();

    let author_name = first_nonempty_str(&[
        header.and_then(|h| h.get("dpn").or_else(|| h.get("displayName")).or_else(|| h.get("name"))),
        sp.get("authorName"),
        sp.get("dpn"),
    ])
    .unwrap_or(if author_uid.is_empty() {
        "Bài viết được chia sẻ"
    } else {
        author_uid.as_str()
    })
    .to_string();

    let sp_content = sp.get("content");
    let content = sp_content
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            first_nonempty_str(&[
                sp_content.and_then(|c| c.get("caption").or_else(|| c.get("msg")).or_else(|| c.get("text"))),
                sp.get("caption"),
                sp.get("msg"),
                sp.get("text"),
            ])
        })
        .unwrap_or("")
        .to_string();

    let link_url = sp
        .get("url")
        .or_else(|| sp_content.and_then(|c| c.get("url")))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    if content.trim().is_empty() && link_url.is_empty() && author_name.is_empty() {
        return None;
    }

    Some(FeedSharedPost {
        author_uid,
        author_name,
        content,
        link_url,
    })
}

fn extract_location_label(item: &Value, content_obj: &Value) -> Option<String> {
    let loc_val = item
        .get("location")
        .or_else(|| content_obj.get("location"))
        .or_else(|| item.get("place"))
        .or_else(|| content_obj.get("checkin"))?;
    if let Some(s) = loc_val.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(s.to_string());
    }
    first_nonempty_str(&[
        loc_val.get("name"),
        loc_val.get("title"),
        loc_val.get("address"),
    ])
    .map(str::to_string)
}

fn extract_feed_comments(item: &Value, footer: &Value) -> Vec<FeedComment> {
    let Some(arr) = item
        .get("comments")
        .or_else(|| footer.get("comments"))
        .or_else(|| item.get("list_comment"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (idx, c) in arr.iter().enumerate() {
        let comment_id = value_as_id_string(c.get("id").or_else(|| c.get("commentId")))
            .unwrap_or_else(|| format!("cmt_{idx}"));
        let author_uid = value_as_id_string(
            c.get("uid")
                .or_else(|| c.get("authorUid"))
                .or_else(|| c.get("userId")),
        )
        .map(|s| normalize_feed_uid(&s))
        .unwrap_or_default();
        let author_name = first_nonempty_str(&[
            c.get("dpn"),
            c.get("authorName"),
            c.get("displayName"),
            c.get("name"),
        ])
        .unwrap_or(author_uid.as_str())
        .to_string();
        let content = first_nonempty_str(&[c.get("content"), c.get("msg"), c.get("text")])
            .unwrap_or("")
            .to_string();
        if content.is_empty() {
            continue;
        }
        let cts = parse_i64_any(
            c.get("cts")
                .or_else(|| c.get("createdTime"))
                .or_else(|| c.get("time")),
        )
        .map(normalize_timestamp_ms)
        .unwrap_or(0);
        out.push(FeedComment {
            comment_id,
            author_name,
            author_uid,
            content,
            timestamp_str: format_vn_timestamp_ms(cts),
        });
    }
    out
}

fn detect_explicit_post_kind(item: &Value, content_obj: &Value) -> Option<FeedPostKind> {
    for candidate in [
        item.get("type"),
        item.get("feedType"),
        item.get("subType"),
        content_obj.get("type"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(s) = candidate.as_str() {
            let low = s.to_ascii_lowercase();
            if low.contains("avatar") {
                return Some(FeedPostKind::AvatarUpdate);
            }
            if low.contains("cover") || low.contains("bgavatar") {
                return Some(FeedPostKind::CoverUpdate);
            }
            if low.contains("share") {
                return Some(FeedPostKind::SharedPost);
            }
            if low.contains("link") {
                return Some(FeedPostKind::Link);
            }
            if low.contains("video") {
                return Some(FeedPostKind::Video);
            }
        }
    }
    None
}

fn parse_feed_item_json(
    val: &Value,
    target_uid: &str,
    rt_stats: &HashMap<String, RealtimeFeedStat>,
) -> Option<FeedItem> {
    let item = val.get("item").unwrap_or(val);
    let header = item.get("header").or_else(|| item.get("author"));

    let author_uid = value_as_id_string(
        header
            .and_then(|h| {
                h.get("uid")
                    .or_else(|| h.get("userId"))
                    .or_else(|| h.get("ownerId"))
            })
            .or_else(|| item.get("uid"))
            .or_else(|| item.get("userId"))
            .or_else(|| item.get("ownerId"))
            .or_else(|| item.get("owner_id"))
            .or_else(|| item.get("authorId")),
    )
    .map(|s| normalize_feed_uid(&s))
    .unwrap_or_default();

    let author_dpn = first_nonempty_str(&[
        header.and_then(|h| h.get("dpn")),
        header.and_then(|h| h.get("displayName")),
        header.and_then(|h| h.get("name")),
        item.get("dpn"),
        item.get("displayName"),
        item.get("authorName"),
    ])
    .unwrap_or("")
    .to_string();

    let author_avt = [
        header.and_then(|h| h.get("avt")),
        header.and_then(|h| h.get("avatar")),
        item.get("avt"),
        item.get("avatar"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::trim)
    .find(|s| is_usable_social_image_url(s))
    .unwrap_or("")
    .to_string();

    let norm_target = normalize_feed_uid(target_uid);
    let matches_target = norm_target.is_empty()
        || (!author_uid.is_empty() && author_uid == norm_target)
        || (author_uid.is_empty() && author_dpn.eq_ignore_ascii_case(target_uid.trim()));
    if !matches_target {
        return None;
    }

    let fid = value_as_id_string(
        item.get("fid")
            .or_else(|| item.get("feedId"))
            .or_else(|| item.get("feed_id"))
            .or_else(|| item.get("postId"))
            .or_else(|| item.get("id")),
    )?;

    let raw_cts = parse_i64_any(
        item.get("cts")
            .or_else(|| item.get("createdTime"))
            .or_else(|| item.get("createTime"))
            .or_else(|| item.get("created_at"))
            .or_else(|| item.get("timestamp"))
            .or_else(|| item.get("ts"))
            .or_else(|| item.get("time")),
    )
    .unwrap_or(0);
    let cts = normalize_timestamp_ms(raw_cts);
    let date_str = format_vn_timestamp_ms(cts);

    let author = FeedAuthor {
        uid: author_uid,
        display_name: author_dpn,
        avatar_url: author_avt,
    };

    let content_val = item.get("content");
    let content_obj = content_val
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));

    let caption = content_val
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            first_nonempty_str(&[
                content_obj.get("caption"),
                content_obj.get("msg"),
                content_obj.get("description"),
                content_obj.get("text"),
                content_obj.get("body"),
                content_obj.get("status"),
                content_obj.get("title"),
                item.get("caption"),
                item.get("desc"),
                item.get("description"),
                item.get("message"),
                item.get("msg"),
                item.get("text"),
            ])
        })
        .unwrap_or("")
        .to_string();

    let default_dim = content_obj
        .get("photo_dimension")
        .or_else(|| content_obj.get("dimension"));
    let mut media_list = Vec::new();
    let mut seen_urls = HashSet::new();
    let mut push_media = |m: FeedMedia| {
        if seen_urls.insert(m.url.clone()) {
            media_list.push(m);
        }
    };

    for source in [
        content_obj.get("photo"),
        content_obj.get("video"),
        content_obj.get("video_info"),
        item.get("video"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(arr) = source.as_array() {
            for p in arr {
                let inner = p.get("photo").unwrap_or(p);
                if let Some(m) = extract_media_from_photo_val(inner, default_dim) {
                    push_media(m);
                }
            }
        } else if let Some(m) = extract_media_from_photo_val(source, default_dim) {
            push_media(m);
        }
    }

    for arr in [
        content_obj.get("photos"),
        content_obj.get("items"),
        content_obj.get("media_items"),
        content_obj.get("medias"),
        content_obj.get("media"),
        content_obj.get("attachments"),
        content_obj.get("list_photo"),
        content_obj.get("videos"),
        content_obj.get("album").and_then(|a| a.get("photos")),
        item.get("photos"),
        item.get("medias"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_array)
    {
        for p in arr {
            let inner = p.get("photo").or_else(|| p.get("video")).unwrap_or(p);
            if let Some(m) = extract_media_from_photo_val(inner, default_dim) {
                push_media(m);
            }
        }
    }

    let link_preview = extract_link_preview(item, &content_obj);
    let shared_post = extract_shared_post(item, &content_obj);
    let location = extract_location_label(item, &content_obj);

    let footer = item.get("footer").cloned().unwrap_or_else(|| json!({}));
    let mut numlike = footer
        .get("numlike")
        .or_else(|| item.get("likeCount"))
        .or_else(|| item.get("numLike"))
        .and_then(|v| parse_u64_any(Some(v)))
        .unwrap_or(0) as u32;
    let mut numcmt = footer
        .get("numcmt")
        .or_else(|| item.get("commentCount"))
        .or_else(|| item.get("numCmt"))
        .and_then(|v| parse_u64_any(Some(v)))
        .unwrap_or(0) as u32;
    let mut is_liked = item
        .get("islike")
        .or_else(|| item.get("isLiked"))
        .and_then(|v| parse_i64_any(Some(v)))
        .unwrap_or(0)
        > 0;

    let mut top_reactions = Vec::new();
    if let Some(rinfo) = footer.get("reaction_info") {
        if rinfo
            .get("current_reaction_id")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            > 0
        {
            is_liked = true;
        }
        if let Some(top_arr) = rinfo.get("top_reactions").and_then(Value::as_array) {
            for r in top_arr {
                if let Some(rid) = r.as_i64() {
                    let label = reaction_id_to_label(rid).to_string();
                    if !top_reactions.contains(&label) {
                        top_reactions.push(label);
                    }
                }
            }
        }
    }
    if top_reactions.is_empty() && numlike > 0 {
        top_reactions.push("Thích".to_string());
    }

    // Ghi đè bằng số liệu real-time trong table_feed_real_time_like_comment nếu có
    if let Some(rt) = rt_stats.get(&fid) {
        if let Some(lk) = rt.like_count {
            numlike = lk;
        }
        if let Some(cm) = rt.comment_count {
            numcmt = cm;
        }
        if let Some(il) = rt.is_liked {
            is_liked = il;
        }
    }

    let comments = extract_feed_comments(item, &footer);
    if numcmt == 0 && !comments.is_empty() {
        numcmt = comments.len() as u32;
    }

    let explicit_kind = detect_explicit_post_kind(item, &content_obj);
    let post_kind = if let Some(k) = explicit_kind {
        k
    } else if shared_post.is_some() {
        FeedPostKind::SharedPost
    } else if media_list.iter().any(|m| m.media_type == "video") {
        FeedPostKind::Video
    } else if media_list.len() > 1 {
        FeedPostKind::Album
    } else if media_list.len() == 1 {
        FeedPostKind::Photo
    } else if link_preview.is_some() {
        FeedPostKind::Link
    } else {
        FeedPostKind::Text
    };

    let privacy = item.get("privacy").and_then(Value::as_i64).unwrap_or(0);
    let status_badge = match privacy {
        2 => Some("Chỉ mình tôi".to_string()),
        1 => Some("Bạn bè".to_string()),
        _ => post_kind.badge_label().map(str::to_string),
    };

    let is_profile_activity = matches!(
        post_kind,
        FeedPostKind::AvatarUpdate | FeedPostKind::CoverUpdate | FeedPostKind::ProfileStatus
    );

    // Avoid blank cards that have no text, no media, no link, and no shared post.
    if caption.is_empty()
        && media_list.is_empty()
        && link_preview.is_none()
        && shared_post.is_none()
    {
        return None;
    }

    Some(FeedItem {
        feed_id: fid,
        author,
        date_str,
        content: caption,
        status_badge,
        media_list,
        reaction: FeedReaction {
            like_count: numlike,
            comment_count: numcmt,
            is_liked,
            top_reactions,
        },
        comments,
        timestamp_ms: cts,
        post_kind,
        link_preview,
        shared_post,
        location,
        is_profile_activity,
    })
}

fn parse_timeline_feed_json(
    val: &Value,
    target_uid: &str,
    rt_stats: &HashMap<String, RealtimeFeedStat>,
) -> Vec<FeedItem> {
    let mut out = Vec::new();
    if let Some(arr) = val.as_array() {
        for elem in arr {
            if let Some(it) = parse_feed_item_json(elem, target_uid, rt_stats) {
                out.push(it);
            }
        }
        return out;
    }

    for list_val in [
        val.get("data").and_then(|d| d.get("items")),
        val.get("data").and_then(|d| d.get("feeds")),
        val.get("items"),
        val.get("feeds"),
        val.get("list"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(arr) = list_val.as_array() {
            for elem in arr {
                if let Some(it) = parse_feed_item_json(elem, target_uid, rt_stats) {
                    out.push(it);
                }
            }
            if !out.is_empty() {
                return out;
            }
        }
    }

    if let Some(it) = parse_feed_item_json(val, target_uid, rt_stats) {
        out.push(it);
    }
    out
}

/// Parse either a single feed post JSON or a timeline array/envelope into normalized `FeedItem`s.
pub fn parse_timeline_feed_value(val: &Value, target_uid: &str) -> Vec<FeedItem> {
    parse_timeline_feed_json(val, target_uid, &HashMap::new())
}

/// Nạp dữ liệu bài viết thật từ bảng `timeline_feed_1`, `user_feed` và hợp nhất số liệu real-time từ `table_feed_real_time_like_comment`
pub fn load_real_timeline_from_sqlite(db_path: &str, target_uid: &str) -> Result<Vec<FeedItem>> {
    let conn = rusqlite::Connection::open(db_path)?;
    let rt_stats = load_realtime_like_comment_map(&conn);

    let mut feeds = Vec::new();
    let mut seen_fids = HashSet::new();

    for table_name in ["timeline_feed_1", "user_feed"] {
        let table_exists: bool = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table_name],
                |row| row.get(0),
            )
            .map(|count: i64| count > 0)
            .unwrap_or(false);

        if !table_exists {
            continue;
        }

        let query = format!("SELECT content FROM {table_name}");
        let mut stmt = conn.prepare(&query)?;
        let rows = stmt.query_map([], |row| {
            let content_str: String = row.get(0)?;
            Ok(content_str)
        })?;

        for r in rows {
            let content_str = r?;
            if let Ok(val) = serde_json::from_str::<Value>(&content_str) {
                for feed_item in parse_timeline_feed_json(&val, target_uid, &rt_stats) {
                    if seen_fids.insert(feed_item.feed_id.clone()) {
                        feeds.push(feed_item);
                    }
                }
            }
        }
    }

    // Sắp xếp bài đăng mới nhất (timestamp_ms lớn nhất) lên đầu danh sách
    feeds.sort_by(|a, b| {
        b.timestamp_ms
            .cmp(&a.timestamp_ms)
            .then_with(|| a.is_profile_activity.cmp(&b.is_profile_activity))
            .then_with(|| b.feed_id.cmp(&a.feed_id))
    });

    Ok(feeds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn social_profile_and_feed_packet_encode_correctly() {
        let payload = packet::build_get_feed_payload(435550501, 435550501, 0, 1, 20);
        assert_eq!(payload.len(), 52, "Payload pw0.e.d phải chuẩn 52 bytes");
        assert_eq!(payload[0], 0x01);
        assert_eq!(
            i32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]),
            packet::CLIENT_VERSION
        );
        assert_eq!(payload[9], 0); // tab = 0
        assert_eq!(
            i32::from_be_bytes([payload[10], payload[11], payload[12], payload[13]]),
            1
        ); // page = 1
        assert_eq!(
            i32::from_be_bytes([payload[14], payload[15], payload[16], payload[17]]),
            20
        ); // count = 20

        let pkt = packet::build_request_packet(
            packet::CMD_FEED_TIMELINE,
            packet::SUBCMD_GET_FEED,
            &payload,
        );
        assert_eq!(pkt.len(), 4 + 16 + 52);
        assert_eq!(pkt[4], 0x01); // version = 1
        assert_eq!(pkt[5], 0x00); // encrypt = 0
        assert_eq!(pkt[10], packet::CMD_TYPE_REQUEST); // cmd_type = 3
        assert_eq!(u16::from_be_bytes([pkt[11], pkt[12]]), 1230); // CMD_FEED_TIMELINE = 1230
        assert_eq!(pkt[13], 6); // SUBCMD_GET_FEED = 6

        let user_payload = packet::build_user_timeline_payload(257404931, 0);
        assert_eq!(user_payload.len(), 18, "Payload mw0.c.k phải chuẩn 18 bytes");

        let prof_val = json!({
            "displayName": "Nguyễn Thành",
            "zaloName": "Thanh",
            "avatar": "https://s120.ava.talk.zdn.vn/a.jpg",
            "cover": "https://cover-talk.zadn.vn/c.jpg",
            "gender": 0,
            "sdob": "01/01/2000",
            "status": "Hz",
            "isFr": 1,
            "lastActionTime": 1762958439000u64
        });
        let p = SocialUserProfile::from_json("435550501", &prof_val).unwrap();
        assert_eq!(p.display_name, "Nguyễn Thành");
        assert!(p.is_friend);
        assert_eq!(p.cover_url, "https://cover-talk.zadn.vn/c.jpg");
        assert_eq!(p.last_action_time, Some(1762958439000u64));
    }

    #[test]
    fn normalize_feed_uid_strips_pversion_suffix_and_preserves_plain_ids() {
        assert_eq!(normalize_feed_uid(" 686454123_0 "), "686454123");
        assert_eq!(normalize_feed_uid("686454123_12"), "686454123");
        assert_eq!(normalize_feed_uid("686454123"), "686454123");
        assert_eq!(normalize_feed_uid("group_12345"), "group_12345");
        assert!(is_raw_uid_label("686454123", "686454123"));
        assert!(is_raw_uid_label("686454123_0", "999999999"));
        assert!(!is_raw_uid_label("Minh Anh", "686454123"));
    }

    #[test]
    fn social_profile_from_json_normalizes_pversion_key_and_filters_default_urls() {
        let prof_val = json!({
            "zaloName": "Bình Minh",
            "avatar": "https://s120.ava.talk.zdn.vn/default",
            "cover": "https://cover-talk.zadn.vn/default.jpg",
            "isFr": 1
        });
        let p = SocialUserProfile::from_json("686454123_0", &prof_val).unwrap();
        assert_eq!(p.uid, "686454123");
        assert_eq!(p.display_name, "Bình Minh");
        assert_eq!(p.avatar_url, "");
        assert_eq!(p.cover_url, "");
    }

    #[test]
    fn resolve_feed_author_never_reattributes_nonempty_uid_by_display_name() {
        let profiles = vec![SocialUserProfile {
            uid: "111111111".to_string(),
            username: String::new(),
            display_name: "Trùng Tên".to_string(),
            zalo_name: "Trùng Tên".to_string(),
            avatar_url: "https://ava.example/111.jpg".to_string(),
            cover_url: String::new(),
            gender: 0,
            sdob: String::new(),
            status: String::new(),
            is_friend: true,
            last_action_time: None,
        }];
        let mut known_contacts = HashMap::new();
        known_contacts.insert(
            "222222222".to_string(),
            FeedProfileFallback {
                uid: "222222222".to_string(),
                display_name: "Trùng Tên".to_string(),
                avatar_url: "https://ava.example/222.jpg".to_string(),
            },
        );

        let mut author = FeedAuthor {
            uid: "222222222_0".to_string(),
            display_name: "222222222".to_string(),
            avatar_url: String::new(),
        };
        resolve_feed_author(&mut author, &profiles, &known_contacts);
        assert_eq!(author.uid, "222222222");
        assert_eq!(author.display_name, "Trùng Tên");
        assert_eq!(author.avatar_url, "https://ava.example/222.jpg");

        // Unknown raw UID must never fall back to showing raw digits
        let mut unknown = FeedAuthor {
            uid: "999888777".to_string(),
            display_name: "999888777".to_string(),
            avatar_url: String::new(),
        };
        resolve_feed_author(&mut unknown, &profiles, &known_contacts);
        assert_eq!(unknown.uid, "999888777");
        assert_eq!(unknown.display_name, "Người dùng Zalo");
    }

    #[test]
    fn filter_feed_item_for_target_matches_strictly_by_normalized_uid() {
        let item_friend_a = FeedItem {
            feed_id: "f1".to_string(),
            author: FeedAuthor {
                uid: "111111111_0".to_string(),
                display_name: "Cùng Tên".to_string(),
                avatar_url: String::new(),
            },
            date_str: String::new(),
            content: "Post A".to_string(),
            status_badge: None,
            media_list: Vec::new(),
            reaction: FeedReaction {
                like_count: 0,
                comment_count: 0,
                is_liked: false,
                top_reactions: Vec::new(),
            },
            comments: Vec::new(),
            timestamp_ms: 1700000000000,
            post_kind: FeedPostKind::Text,
            link_preview: None,
            shared_post: None,
            location: None,
            is_profile_activity: false,
        };
        assert!(filter_feed_item_for_target(&item_friend_a, ""));
        assert!(filter_feed_item_for_target(&item_friend_a, "111111111"));
        assert!(!filter_feed_item_for_target(&item_friend_a, "222222222"));
    }

    #[test]
    fn parse_timeline_feed_supports_standard_post_types() {
        let feed_payload = json!({
            "data": {
                "items": [
                    {
                        "fid": 1001,
                        "uid": "100001_0",
                        "dpn": "Lan Anh",
                        "cts": 1762958439,
                        "content": "Trạng thái thuần văn bản buổi sáng!",
                        "location": { "name": "Hà Nội" },
                        "likeCount": 5,
                        "comments": [
                            { "id": "c1", "uid": "100002", "dpn": "Tuấn", "content": "Chào buổi sáng!", "cts": 1762958500 }
                        ]
                    },
                    {
                        "fid": "1002",
                        "header": { "uid": "100002", "dpn": "Tuấn", "avt": "https://ava.example/t.jpg" },
                        "cts": 1762958445000i64,
                        "content": {
                            "caption": "Video du lịch Đà Lạt",
                            "video": {
                                "photoid": "v1",
                                "videourl": "https://vid.example/dalat.mp4",
                                "thumb": "https://img.example/dalat_thumb.jpg",
                                "width": 1920,
                                "height": 1080,
                                "duration": 95
                            }
                        }
                    },
                    {
                        "fid": "1003",
                        "header": { "uid": "100003", "dpn": "Hùng" },
                        "cts": 1762958450000i64,
                        "content": {
                            "caption": "Bài viết hay nên đọc",
                            "link": {
                                "url": "https://news.example.vn/post-1",
                                "title": "Tin công nghệ",
                                "desc": "Tóm tắt nội dung bài báo",
                                "thumb": "https://news.example.vn/thumb.jpg"
                            }
                        }
                    },
                    {
                        "fid": "1004",
                        "header": { "uid": "100004", "dpn": "Mai" },
                        "cts": 1762958460000i64,
                        "content": {
                            "caption": "Chia sẻ lại kỷ niệm",
                            "photos": [
                                { "url": "https://img.example/1.jpg", "width": 800, "height": 600 },
                                { "url": "https://img.example/2.jpg", "width": 800, "height": 1000 }
                            ]
                        },
                        "share_feed": {
                            "uid": "100001",
                            "authorName": "Lan Anh",
                            "content": "Bài gốc từ năm ngoái"
                        }
                    }
                ]
            }
        });

        let parsed = parse_timeline_feed_value(&feed_payload, "");
        assert_eq!(parsed.len(), 4);

        assert_eq!(parsed[0].feed_id, "1001");
        assert_eq!(parsed[0].author.uid, "100001");
        assert_eq!(parsed[0].post_kind, FeedPostKind::Text);
        assert_eq!(parsed[0].location.as_deref(), Some("Hà Nội"));
        assert_eq!(parsed[0].timestamp_ms, 1762958439000);
        assert_eq!(parsed[0].comments.len(), 1);
        assert_eq!(parsed[0].reaction.comment_count, 1);

        assert_eq!(parsed[1].feed_id, "1002");
        assert_eq!(parsed[1].post_kind, FeedPostKind::Video);
        assert_eq!(parsed[1].media_list.len(), 1);
        assert_eq!(parsed[1].media_list[0].duration_secs, Some(95));
        assert_eq!(parsed[1].media_list[0].width, 1920);
        assert_eq!(parsed[1].media_list[0].height, 1080);

        assert_eq!(parsed[2].feed_id, "1003");
        assert_eq!(parsed[2].post_kind, FeedPostKind::Link);
        let lp = parsed[2].link_preview.as_ref().expect("link preview");
        assert_eq!(lp.domain, "news.example.vn");
        assert_eq!(lp.title, "Tin công nghệ");

        assert_eq!(parsed[3].feed_id, "1004");
        assert_eq!(parsed[3].post_kind, FeedPostKind::SharedPost);
        assert_eq!(parsed[3].media_list.len(), 2);
        let sp = parsed[3].shared_post.as_ref().expect("shared post");
        assert_eq!(sp.author_name, "Lan Anh");
    }
}
