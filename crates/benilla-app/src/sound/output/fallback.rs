//! Which outputs stand in for a default output that will not open, decided from names alone so
//! it is tested on every platform; `cpal.rs` applies it to the devices its host enumerates.

/// Where an ALSA PCM ranks as a stand-in for `default`, by its hint name and description, lowest
/// first; `None` is never tried.
///
/// - A sound server's PCM first (`pipewire`, then `pulse`): when one holds the cards, a `default`
///   left as the stock `dmix` cannot open its slave, and the server's PCM is what plays.
/// - Then each card's own default (`sysdefault:CARD=…`), then the card with conversions
///   (`plughw:CARD=…`): the cards themselves when no server holds them.
/// - HDMI and DisplayPort cards after the rest: a monitor often has no speakers.
/// - Never `default` (already tried) or `null` (accepts everything, plays nothing); nor a card's
///   other routes (`hw`, `dmix`, `front`, `surround*`, `iec958`), which reach the same card as its
///   `sysdefault` and `plughw`, nor the capture, rate and mix plugins, nor `jack`.
pub(super) fn alsa_rank(pcm: &str, desc: &str) -> Option<u8> {
    let (kind, args) = pcm.split_once(':').unwrap_or((pcm, ""));
    let display = [pcm, desc].iter().any(|s| {
        let s = s.to_ascii_lowercase();
        s.contains("hdmi") || s.contains("displayport")
    });
    match (kind, args.is_empty()) {
        ("pipewire", true) => Some(0),
        ("pulse", true) => Some(1),
        ("sysdefault" | "default", false) => Some(if display { 4 } else { 2 }),
        ("plughw", false) => Some(if display { 5 } else { 3 }),
        _ => None,
    }
}

/// `items` lowest rank first, in their given order within a rank, without the unranked.
pub(super) fn by_rank<T>(
    items: impl IntoIterator<Item = T>,
    rank: impl Fn(&T) -> Option<u8>,
) -> Vec<T> {
    let mut ranked: Vec<(u8, T)> = items
        .into_iter()
        .filter_map(|item| rank(&item).map(|r| (r, item)))
        .collect();
    ranked.sort_by_key(|(r, _)| *r);
    ranked.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(hints: &[(&'static str, &'static str)]) -> Vec<&'static str> {
        by_rank(hints.iter().copied(), |(pcm, desc)| alsa_rank(pcm, desc))
            .into_iter()
            .map(|(pcm, _)| pcm)
            .collect()
    }

    /// An `aplay -L` with PipeWire's and Pulse's PCMs, an onboard card and an NVIDIA HDMI card,
    /// each hint's description cut to its first line.
    #[test]
    fn a_sound_server_comes_before_the_cards() {
        let hints = [
            (
                "null",
                "Discard all samples (playback) or generate zero samples (capture)",
            ),
            ("hw:CARD=NVidia,DEV=3", "HDA NVidia, HDMI 0"),
            ("plughw:CARD=NVidia,DEV=3", "HDA NVidia, HDMI 0"),
            ("sysdefault:CARD=NVidia", "HDA NVidia, HDMI 0"),
            ("hdmi:CARD=NVidia,DEV=0", "HDA NVidia, HDMI 0"),
            ("pulse", "PulseAudio Sound Server"),
            (
                "default",
                "Default ALSA Output (currently PipeWire Media Server)",
            ),
            ("pipewire", "PipeWire Sound Server"),
            ("hw:CARD=PCH,DEV=0", "HDA Intel PCH, ALC892 Analog"),
            ("plughw:CARD=PCH,DEV=0", "HDA Intel PCH, ALC892 Analog"),
            ("sysdefault:CARD=PCH", "HDA Intel PCH, ALC892 Analog"),
            ("front:CARD=PCH,DEV=0", "HDA Intel PCH, ALC892 Analog"),
            ("surround51:CARD=PCH,DEV=0", "HDA Intel PCH, ALC892 Analog"),
            ("dmix:CARD=PCH,DEV=0", "HDA Intel PCH, ALC892 Analog"),
            ("dsnoop:CARD=PCH,DEV=0", "HDA Intel PCH, ALC892 Analog"),
            ("iec958:CARD=PCH,DEV=0", "HDA Intel PCH, ALC892 Digital"),
        ];
        assert_eq!(
            order(&hints),
            [
                "pipewire",
                "pulse",
                "sysdefault:CARD=PCH",
                "plughw:CARD=PCH,DEV=0",
                "sysdefault:CARD=NVidia",
                "plughw:CARD=NVidia,DEV=3",
            ]
        );
    }

    /// A PipeWire system whose `default` is left as the stock card route, as one player's
    /// `aplay -L` lists it: the plugins ahead of `pipewire` are never stand-ins.
    #[test]
    fn the_plugins_listed_first_are_passed_over() {
        let hints = [
            (
                "null",
                "Discard all samples (playback) or generate zero samples (capture)",
            ),
            (
                "lavrate",
                "Rate Converter Plugin Using Libav/FFmpeg Library",
            ),
            (
                "samplerate",
                "Rate Converter Plugin Using Samplerate Library",
            ),
            ("speexrate", "Rate Converter Plugin Using Speex Resampler"),
            ("jack", "JACK Audio Connection Kit"),
            ("oss", "Open Sound System"),
            ("pipewire", "PipeWire Sound Server"),
            ("pulse", "PulseAudio Sound Server"),
            (
                "speex",
                "Plugin using Speex DSP (resample, agc, denoise, echo, dereverb)",
            ),
            ("upmix", "Plugin for channel upmix (4,6,8)"),
            (
                "vdownmix",
                "Plugin for channel downmix (stereo) with a simple spacialization",
            ),
            ("hdmi:CARD=NVidia,DEV=0", "HDA NVidia, PL2288H"),
            ("hdmi:CARD=NVidia,DEV=1", "HDA NVidia, HDMI 1"),
        ];
        assert_eq!(order(&hints), ["pipewire", "pulse"]);
    }

    /// Two cards and no sound server's PCM: each card's default before either card raw, in the
    /// host's card order.
    #[test]
    fn without_a_server_each_cards_default_comes_first() {
        let hints = [
            ("default", "Default Audio Device"),
            (
                "sysdefault:CARD=Generic",
                "HD-Audio Generic, ALC1220 Analog",
            ),
            (
                "plughw:CARD=Generic,DEV=0",
                "HD-Audio Generic, ALC1220 Analog",
            ),
            ("sysdefault:CARD=Audio", "USB Audio, USB Audio"),
            ("plughw:CARD=Audio,DEV=0", "USB Audio, USB Audio"),
            (
                "lavrate",
                "Rate Converter Plugin Using Libav/FFmpeg Library",
            ),
            ("jack", "JACK Audio Connection Kit"),
        ];
        assert_eq!(
            order(&hints),
            [
                "sysdefault:CARD=Generic",
                "sysdefault:CARD=Audio",
                "plughw:CARD=Generic,DEV=0",
                "plughw:CARD=Audio,DEV=0",
            ]
        );
    }

    #[test]
    fn the_default_and_silent_sinks_are_never_stand_ins() {
        for pcm in ["default", "null", "pipewire:NODE=x", "sysdefault", "plughw"] {
            assert_eq!(alsa_rank(pcm, ""), None, "{pcm}");
        }
    }
}
