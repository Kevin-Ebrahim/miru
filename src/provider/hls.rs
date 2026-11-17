use super::parser;
use crate::{
    model::{Audio, Mode, Variant},
    network::http_url,
};
use anyhow::{Context, Result, bail, ensure};
use std::collections::HashMap;
use url::Url;

fn hls_attributes(mut list: &str) -> Result<HashMap<&str, &str>> {
    let mut attributes = HashMap::new();
    while !list.is_empty() {
        let (name, value) = list
            .split_once('=')
            .context("Malformed HLS attribute list")?;
        let name = name.trim();
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-'),
            "Invalid HLS attribute name"
        );
        let value = value.trim_start();
        let (value, rest) = if let Some(quoted) = value.strip_prefix('"') {
            let (value, rest) = quoted
                .split_once('"')
                .context("Unterminated HLS quoted attribute")?;
            let rest = rest.trim_start();
            let rest = if rest.is_empty() {
                rest
            } else {
                rest.strip_prefix(',')
                    .context("Malformed HLS attribute separator")?
            };
            (value, rest)
        } else {
            let (value, rest) = value.split_once(',').unwrap_or((value, ""));
            (value.trim(), rest)
        };
        ensure!(
            attributes.insert(name, value).is_none(),
            "Duplicate HLS attribute {name:?}"
        );
        list = rest.trim_start();
    }
    Ok(attributes)
}

fn audio_only_codecs(codecs: &str) -> bool {
    // Registered audio sample entries: https://mp4ra.org/registered-types/codecs
    // Unknown codec families remain eligible instead of guessing that video is absent.
    codecs.split(',').all(|codec| {
        matches!(
            codec.trim().split('.').next(),
            Some(
                "mp4a"
                    | "ac-3"
                    | "ec-3"
                    | "ac-4"
                    | "alac"
                    | "Opus"
                    | "fLaC"
                    | "dtsc"
                    | "dtse"
                    | "dtsh"
                    | "dtsl"
            )
        )
    })
}

pub(super) fn parse_playlist(text: &str, master: &Url, mode: Option<Mode>) -> Result<Vec<Variant>> {
    ensure!(
        text.trim_start().starts_with("#EXTM3U"),
        "Stream host did not return an HLS playlist"
    );
    ensure!(
        text.len() <= parser::BODY_LIMIT,
        "HLS playlist exceeds size limit"
    );
    struct Rendition {
        language: Option<Mode>,
        is_default: bool,
        autoselect: bool,
        url: Option<String>,
    }
    // Shared renditions and relative URLs can expand far beyond the input text.
    let mut remaining = parser::BODY_LIMIT;
    let mut reserve = |bytes: usize| -> Result<()> {
        remaining = remaining
            .checked_sub(bytes)
            .context("Resolved HLS rendition data exceeds size limit")?;
        Ok(())
    };
    let mut groups: HashMap<&str, Vec<Rendition>> = HashMap::new();
    for line in text.lines().map(str::trim) {
        let Some(list) = line.strip_prefix("#EXT-X-MEDIA:") else {
            continue;
        };
        let attributes = hls_attributes(list)?;
        if attributes.get("TYPE") != Some(&"AUDIO") {
            continue;
        }
        let group = attributes
            .get("GROUP-ID")
            .copied()
            .filter(|group| !group.is_empty())
            .context("HLS audio rendition has no group identity")?;
        let rendition = Rendition {
            language: attributes.get("LANGUAGE").and_then(|value| language(value)),
            is_default: attributes.get("DEFAULT") == Some(&"YES"),
            autoselect: attributes.get("AUTOSELECT") == Some(&"YES"),
            url: attributes
                .get("URI")
                .map(|uri| -> Result<String> {
                    let url = http_url(master.join(uri)?.as_str())?.to_string();
                    reserve(url.len())?;
                    Ok(url)
                })
                .transpose()?,
        };
        let renditions = groups.entry(group).or_default();
        ensure!(
            !rendition.is_default || !renditions.iter().any(|current| current.is_default),
            "Multiple default audio renditions in HLS group {group:?}"
        );
        renditions.push(rendition);
    }
    let groups: HashMap<_, _> = groups
        .into_iter()
        .map(|(group, renditions)| {
            let mut selected: Option<Rendition> = None;
            for rendition in renditions {
                if mode.is_some() && rendition.language != mode {
                    continue;
                }
                if selected.as_ref().is_none_or(|current| {
                    (rendition.is_default, rendition.autoselect)
                        > (current.is_default, current.autoselect)
                }) {
                    selected = Some(rendition);
                }
            }
            (group, selected)
        })
        .collect();
    let mut variants = Vec::new();
    let mut pending = None;
    let mut excluded_audio = false;
    for line in text.lines().map(str::trim) {
        if let Some(list) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let attributes = hls_attributes(list)?;
            let height = attributes
                .get("RESOLUTION")
                .and_then(|resolution| resolution.split_once('x'))
                .and_then(|(width, height)| {
                    width.parse::<u32>().ok()?;
                    height.parse().ok()
                })
                .unwrap_or(0);
            if height == 0
                && attributes
                    .get("CODECS")
                    .is_some_and(|codecs| audio_only_codecs(codecs))
            {
                excluded_audio = true;
                pending = None;
                continue;
            }
            let audio = attributes
                .get("AUDIO")
                .map(|&group| -> Result<Audio> {
                    let rendition = groups
                        .get(group)
                        .with_context(|| {
                            format!("HLS variant references missing audio group {group:?}")
                        })?
                        .as_ref()
                        .with_context(|| {
                            format!(
                                "Requested {} audio is unavailable in HLS group {group:?}",
                                mode.map_or("default", |m| m.as_str())
                            )
                        })?;
                    ensure!(
                        mode.is_none() || rendition.url.is_some(),
                        "Requested audio is embedded; explicit track selection is unavailable"
                    );
                    reserve(group.len())?;
                    if let Some(url) = &rendition.url {
                        reserve(url.len())?;
                    }
                    Ok(Audio {
                        group: group.into(),
                        url: rendition.url.clone(),
                    })
                })
                .transpose()?;
            ensure!(
                mode.is_none() || audio.is_some(),
                "HLS variant does not identify the requested audio language"
            );
            pending = Some((height, audio));
        } else if !line.is_empty()
            && !line.starts_with('#')
            && let Some((height, audio)) = pending.take()
        {
            let url = http_url(master.join(line)?.as_str())?.to_string();
            reserve(url.len())?;
            variants.push(Variant {
                format: crate::model::MediaFormat::Hls,
                label: None,
                height,
                url,
                audio,
            });
        }
    }
    if variants.is_empty() && excluded_audio {
        bail!("Audio-only HLS sources are unsupported; a video stream is required");
    }
    if variants.is_empty() && text.contains("#EXTINF:") {
        ensure!(
            mode.is_none(),
            "Media playlist does not identify the requested audio language"
        );
        reserve(master.as_str().len())?;
        variants.push(Variant {
            format: crate::model::MediaFormat::Hls,
            label: None,
            height: 0,
            url: master.to_string(),
            audio: None,
        });
    }

    ensure!(
        !variants.is_empty(),
        "No playable streams in the HLS playlist"
    );
    variants.sort_by_key(|v| std::cmp::Reverse(v.height));
    Ok(variants)
}

fn language(value: &str) -> Option<Mode> {
    let value = value.trim().to_ascii_lowercase().replace('_', "-");
    match value.split('-').next()? {
        "ja" | "jpn" | "japanese" => Some(Mode::Sub),
        "en" | "eng" | "english" => Some(Mode::Dub),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Stream;

    fn parse_playlist(text: &str, master: &Url) -> Result<Vec<Variant>> {
        super::parse_playlist(text, master, None)
    }

    #[test]
    fn explicit_language_overrides_order_and_default_without_changing_legacy_selection() {
        let master = Url::parse("https://cdn.example/title/master.m3u8").unwrap();
        let fixture = include_str!("../../tests/fixtures/multilingual.m3u8");
        for text in [
            fixture.to_owned(),
            fixture.replace("LANGUAGE=\"English\"", "DEFAULT=YES,LANGUAGE=\"English\""),
        ] {
            for (mode, expected) in [(Mode::Sub, "audio-ja"), (Mode::Dub, "audio-en")] {
                let variants = super::parse_playlist(&text, &master, Some(mode)).unwrap();
                assert_eq!(variants.len(), 2);
                for variant in variants {
                    assert_eq!(
                        variant.audio_url(),
                        Some(
                            format!("https://cdn.example/title/{expected}/playlist.m3u8").as_str()
                        )
                    );
                }
            }
            assert_eq!(
                parse_playlist(&text, &master).unwrap()[0].audio_url(),
                Some("https://cdn.example/title/audio-en/playlist.m3u8")
            );
        }
    }

    #[test]
    fn language_aliases_select_explicit_audio() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        for (mode, aliases) in [
            (Mode::Sub, ["ja", "ja-JP", "jpn", "Japanese", "JA_jp"]),
            (Mode::Dub, ["en", "en-US", "eng", "English", "EN_gb"]),
        ] {
            for alias in aliases {
                let text = format!(
                    "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",LANGUAGE=\"{alias}\",URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:RESOLUTION=640x480,AUDIO=\"a\"\nvideo.m3u8"
                );
                assert!(
                    super::parse_playlist(&text, &master, Some(mode)).is_ok(),
                    "{alias}"
                );
                let other = if mode == Mode::Sub {
                    Mode::Dub
                } else {
                    Mode::Sub
                };
                assert!(
                    super::parse_playlist(&text, &master, Some(other)).is_err(),
                    "{alias}"
                );
            }
        }
    }

    #[test]
    fn explicit_language_never_guesses_from_names_embedded_audio_or_other_groups() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let fixture = include_str!("../../tests/fixtures/multilingual.m3u8");
        for text in [
            fixture.replace("LANGUAGE=\"Japanese\"", "LANGUAGE=\"und\""),
            fixture.replace("LANGUAGE=\"Japanese\"", "LANGUAGE=\"fr\""),
            fixture.replace("LANGUAGE=\"Japanese\",", ""),
            fixture.replace(",URI=\"audio-ja/playlist.m3u8\"", ""),
            fixture.replace(
                "GROUP-ID=\"stereo\",NAME=\"Japanese",
                "GROUP-ID=\"unused\",NAME=\"Japanese",
            ),
            fixture.replace(",AUDIO=\"stereo\"", ""),
            "#EXTM3U\n#EXTINF:1,\nsegment.ts".into(),
        ] {
            assert!(
                super::parse_playlist(&text, &master, Some(Mode::Sub)).is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn hls_resolves_relative_urls_audio_and_quality() {
        let master = Url::parse("https://cdn.example/path/master.m3u8?token=test").unwrap();
        let variants =
            parse_playlist(include_str!("../../tests/fixtures/master.m3u8"), &master).unwrap();
        assert_eq!(
            variants.iter().map(|v| v.height).collect::<Vec<_>>(),
            [1080, 720, 360]
        );
        assert_eq!(variants[1].url, "https://cdn.example/path/720/index.m3u8");
        assert_eq!(
            variants[0].audio_url(),
            Some("https://cdn.example/audio/index.m3u8")
        );
        assert_eq!(variants[1].audio_url(), variants[0].audio_url());
        assert!(variants[2].audio.is_none());
        let stream = Stream {
            variants,
            subtitle: None,
            referer: String::new(),
        };
        assert_eq!(stream.select("worst").0.height, 360);
        assert_eq!(stream.select("720p").0.height, 720);
        assert!(stream.select("480p").1);
    }

    #[test]
    fn media_playlist_and_invalid_responses() {
        let url = Url::parse("https://example.org/a.m3u8").unwrap();
        assert_eq!(
            parse_playlist("#EXTM3U\n#EXTINF:2.0,\nseg.ts", &url)
                .unwrap()
                .len(),
            1
        );
        assert!(parse_playlist("<html>error</html>", &url).is_err());
        assert!(http_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn selected_video_keeps_its_audio_group_in_player_arguments() {
        let master = Url::parse("https://cdn.example/path/master.m3u8").unwrap();
        let variants = parse_playlist(
            include_str!("../../tests/fixtures/audio-groups.m3u8"),
            &master,
        )
        .unwrap();
        let stream = Stream {
            variants,
            subtitle: Some("https://cdn.example/subtitle.vtt".into()),
            referer: String::new(),
        };
        let exported = serde_json::to_value(&stream).unwrap();
        assert_eq!(exported["audio"], "https://cdn.example/path/first.m3u8");
        assert_eq!(exported["referer"], "");
        assert_eq!(exported["subtitle"], "https://cdn.example/subtitle.vtt");
        assert_eq!(exported["variants"][0]["height"], 1080);
        assert_eq!(
            exported["variants"][0]["url"],
            "https://cdn.example/path/video-high.m3u8"
        );
        assert_eq!(exported["variants"][0]["audio"]["group"], "a1");
        assert_eq!(exported["variants"][0]["audio"]["url"], exported["audio"]);
        assert_eq!(exported["variants"][1]["audio"]["group"], "a2");
        for (quality, video, audio) in [
            ("best", "video-high.m3u8", "first.m3u8"),
            ("1080p", "video-high.m3u8", "first.m3u8"),
            ("worst", "video-low.m3u8", "second.m3u8"),
            ("480", "video-low.m3u8", "second.m3u8"),
            ("720p", "video-high.m3u8", "first.m3u8"),
        ] {
            for player in ["mpv", "vlc"] {
                let arguments = crate::player::Player::new(player, vec![], None)
                    .args(&stream, quality, "Probe");
                let audio = format!("https://cdn.example/path/{audio}");
                let expected = if player == "vlc" {
                    format!(":input-slave={audio}#https://cdn.example/subtitle.vtt")
                } else {
                    format!("--audio-file={audio}")
                };
                assert!(
                    arguments.contains(&expected),
                    "{player} {quality}: {arguments:?}"
                );
                assert!(arguments.contains(&format!("https://cdn.example/path/{video}")));
            }
        }
    }

    #[test]
    fn hls_audio_groups_can_follow_video_and_contain_quoted_commas() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let variants = parse_playlist(
            r#"#EXTM3U
            #EXT-X-STREAM-INF:RESOLUTION=1920x1080,AUDIO="late,group"
            video.m3u8
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="late,group",NAME="DEFAULT=YES, stereo",URI="other.m3u8"
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="late,group",NAME="Chosen",DEFAULT=YES,URI="chosen.m3u8?token=x,y"
            "#,
            &master,
        )
        .unwrap();
        assert_eq!(
            variants[0].audio_url(),
            Some("https://cdn.example/chosen.m3u8?token=x,y")
        );
        assert_eq!(variants[0].audio.as_ref().unwrap().group, "late,group");
    }

    #[test]
    fn mixed_hls_selects_video_for_best_and_worst() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let stream = Stream {
            variants: parse_playlist(
                include_str!("../../tests/fixtures/mixed-media.m3u8"),
                &master,
            )
            .unwrap(),
            subtitle: None,
            referer: String::new(),
        };
        for (quality, video) in [("best", "video-high.m3u8"), ("worst", "video-low.m3u8")] {
            let expected = format!("https://cdn.example/{video}");
            assert_eq!(stream.select(quality).0.url, expected);
            for player in ["mpv", "vlc"] {
                assert!(
                    crate::player::Player::new(player, vec![], None)
                        .args(&stream, quality, "Probe")
                        .contains(&expected)
                );
            }
        }
        assert_eq!(
            serde_json::to_value(stream).unwrap()["variants"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn unknown_video_resolution_and_direct_media_remain_eligible() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        for codecs in [
            None,
            Some("avc1.4d401f,mp4a.40.2"),
            Some("future.1,mp4a.40.2"),
            Some(""),
        ] {
            let codecs = codecs
                .map(|value| format!(",CODECS=\"{value}\""))
                .unwrap_or_default();
            let text = format!(
                "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Main\",DEFAULT=YES,URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=200000,AUDIO=\"a\"{codecs}\nvideo.m3u8\n"
            );
            let stream = Stream {
                variants: parse_playlist(&text, &master).unwrap(),
                subtitle: None,
                referer: String::new(),
            };
            for quality in ["best", "worst"] {
                let (variant, _) = stream.select(quality);
                assert_eq!(variant.url, "https://cdn.example/video.m3u8");
                assert_eq!(variant.height, 0);
                assert_eq!(variant.audio_url(), Some("https://cdn.example/audio.m3u8"));
            }
        }
        let direct =
            parse_playlist("#EXTM3U\n#EXTINF:1,\nsegment.ts\n#EXT-X-ENDLIST\n", &master).unwrap();
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].url, master.as_str());
        assert_eq!(direct[0].height, 0);
    }

    #[test]
    fn audio_only_masters_report_an_unsupported_source() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        for codecs in [
            "mp4a.40.2",
            "ac-3",
            "ec-3",
            "ac-4.02.01.01",
            "Opus",
            "fLaC",
            "alac",
            "dtsc",
            "mp4a.40.2,ec-3",
        ] {
            let text = format!(
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=65000,CODECS=\"{codecs}\"\naudio.m3u8\n"
            );
            let error = parse_playlist(&text, &master).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Audio-only HLS sources are unsupported"),
                "{codecs}: {error}"
            );
        }
    }

    #[test]
    fn hls_optional_default_keeps_the_available_audio_rendition() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let expected = "https://cdn.example/audio.m3u8";
        for default in ["DEFAULT=YES,", "", "DEFAULT=NO,"] {
            let text = format!(
                "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Main\",AUTOSELECT=YES,{default}URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:RESOLUTION=128x96,AUDIO=\"a\"\nvideo.m3u8\n"
            );
            let stream = Stream {
                variants: parse_playlist(&text, &master).unwrap(),
                subtitle: None,
                referer: String::new(),
            };
            assert_eq!(stream.select("best").0.audio_url(), Some(expected));
            assert_eq!(serde_json::to_value(&stream).unwrap()["audio"], expected);
            for player in ["mpv", "vlc"] {
                let arguments =
                    crate::player::Player::new(player, vec![], None).args(&stream, "best", "Probe");
                let audio = if player == "vlc" {
                    format!(":input-slave={expected}")
                } else {
                    format!("--audio-file={expected}")
                };
                assert!(arguments.contains(&audio), "{default:?}: {arguments:?}");
            }
        }
    }

    #[test]
    fn hls_audio_priority_is_default_then_autoselect_then_first() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        for renditions in [
            [
                ("AUTOSELECT=NO", "other.m3u8"),
                ("AUTOSELECT=YES", "audio.m3u8"),
            ],
            [
                ("AUTOSELECT=YES", "other.m3u8"),
                ("DEFAULT=YES,AUTOSELECT=YES", "audio.m3u8"),
            ],
            [
                ("AUTOSELECT=YES", "audio.m3u8"),
                ("AUTOSELECT=YES", "other.m3u8"),
            ],
        ] {
            let mut text = "#EXTM3U\n".to_owned();
            for (index, (flags, uri)) in renditions.into_iter().enumerate() {
                text.push_str(&format!(
                    "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Track {index}\",{flags},URI=\"{uri}\"\n"
                ));
            }
            text.push_str("#EXT-X-STREAM-INF:RESOLUTION=128x96,AUDIO=\"a\"\nvideo.m3u8\n");
            let variants = parse_playlist(&text, &master).unwrap();
            assert_eq!(
                variants[0].audio_url(),
                Some("https://cdn.example/audio.m3u8")
            );
        }
    }

    #[test]
    fn hls_missing_defaults_and_embedded_audio_stay_within_their_groups() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let variants = parse_playlist(
            r#"#EXTM3U
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="first",NAME="First",URI="first.m3u8"
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="first",NAME="Second",URI="second.m3u8"
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="embedded",NAME="Alternate",URI="other.m3u8"
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="embedded",NAME="Embedded",DEFAULT=YES
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="unused",NAME="Unrelated",DEFAULT=YES,URI="unrelated.m3u8"
            #EXT-X-STREAM-INF:RESOLUTION=1920x1080,AUDIO="first"
            high.m3u8
            #EXT-X-STREAM-INF:RESOLUTION=1280x720,AUDIO="embedded"
            medium.m3u8
            #EXT-X-STREAM-INF:RESOLUTION=640x360
            low.m3u8
            "#,
            &master,
        )
        .unwrap();
        assert_eq!(
            variants[0].audio_url(),
            Some("https://cdn.example/first.m3u8")
        );
        assert_eq!(variants[1].audio.as_ref().unwrap().group, "embedded");
        assert!(variants[1].audio_url().is_none());
        assert!(variants[2].audio.is_none());
        let stream = Stream {
            variants,
            subtitle: None,
            referer: String::new(),
        };
        for quality in ["720p", "worst"] {
            for player in ["mpv", "vlc"] {
                let arguments = crate::player::Player::new(player, vec![], None)
                    .args(&stream, quality, "Probe");
                assert!(arguments.iter().all(|argument| {
                    !argument.starts_with("--audio-file=") && !argument.starts_with(":input-slave=")
                }));
            }
        }
        let embedded = Stream {
            variants: vec![stream.variants[1].clone()],
            subtitle: None,
            referer: String::new(),
        };
        let exported = serde_json::to_value(embedded).unwrap();
        assert!(exported["audio"].is_null());
        assert_eq!(exported["variants"][0]["audio"]["group"], "embedded");
        assert!(exported["variants"][0]["audio"]["url"].is_null());
    }

    #[test]
    fn hls_rejects_ambiguous_or_invalid_audio_group_definitions() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let fixture = include_str!("../../tests/fixtures/audio-groups.m3u8");
        for (text, reason) in [
            (
                fixture.replace("AUDIO=\"a1\"", "AUDIO=\"missing\""),
                "missing audio group",
            ),
            (
                fixture.replace("GROUP-ID=\"a2\"", "GROUP-ID=\"a1\""),
                "Multiple default audio",
            ),
            (fixture.replace("GROUP-ID=\"a1\",", ""), "no group identity"),
            (
                fixture.replace("AUDIO=\"a1\"", "AUDIO=\"a1\",AUDIO=\"a2\""),
                "Duplicate HLS attribute",
            ),
            (
                "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"unterminated".into(),
                "Unterminated HLS",
            ),
        ] {
            let error = parse_playlist(&text, &master).unwrap_err();
            assert!(error.to_string().contains(reason), "{error:#}");
        }
    }

    #[test]
    fn hls_shared_renditions_cannot_expand_beyond_the_metadata_budget() {
        let master = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let mut playlist = format!(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"shared\",NAME=\"Large\",DEFAULT=YES,URI=\"{}.m3u8\"\n",
            "a".repeat(128 * 1024)
        );
        for index in 0..40 {
            playlist.push_str(&format!(
                "#EXT-X-STREAM-INF:RESOLUTION=1920x1080,AUDIO=\"shared\"\nvideo-{index}.m3u8\n"
            ));
        }
        assert!(playlist.len() < parser::BODY_LIMIT);
        let result = parse_playlist(&playlist, &master);
        assert!(
            result.is_err(),
            "Shared audio expanded beyond the metadata budget"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("rendition data exceeds")
        );
    }
}
