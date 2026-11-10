//! Bundled fonts (SIL OFL 1.1, license files next to them) and SVG icons.

use std::borrow::Cow;

use gpui::{App, AssetSource, Result, SharedString};

macro_rules! bundle {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../assets/", $path)) as &[u8])),*]
    };
}

const FILES: &[(&str, &[u8])] = bundle![
    "icons/attach.svg",
    "icons/back.svg",
    "icons/branch.svg",
    "icons/capture.svg",
    "icons/chevron-down.svg",
    "icons/chevron-up.svg",
    "icons/close.svg",
    "icons/file.svg",
    "icons/code.svg",
    "icons/device-daemon.svg",
    "icons/device-phone.svg",
    "icons/inbox.svg",
    "icons/link.svg",
    "icons/logo.svg",
    "icons/mail.svg",
    "icons/mic.svg",
    "icons/open.svg",
    "icons/panel.svg",
    "icons/play.svg",
    "icons/pause.svg",
    "icons/repos.svg",
    "icons/search.svg",
    "icons/send.svg",
    "icons/settings.svg",
    "icons/stop.svg",
    "icons/tasks.svg",
    "icons/twist-down.svg",
    "icons/twist-right.svg",
    "icons/warroom.svg",
];

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-SemiBold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    // The Scipio brand face in three static cuts (theme::DISPLAY*).
    include_bytes!("../assets/fonts/BricolageGrotesqueTitle.ttf"),
    include_bytes!("../assets/fonts/BricolageGrotesqueFigure.ttf"),
    include_bytes!("../assets/fonts/BricolageGrotesqueHeading.ttf"),
    // Console fonts of the Display popover.
    include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Bold.ttf"),
    include_bytes!("../assets/fonts/FiraCode-Regular.ttf"),
    include_bytes!("../assets/fonts/FiraCode-Bold.ttf"),
    include_bytes!("../assets/fonts/SourceCodePro-Regular.ttf"),
    include_bytes!("../assets/fonts/SourceCodePro-Bold.ttf"),
    include_bytes!("../assets/fonts/RobotoMono-Regular.ttf"),
    include_bytes!("../assets/fonts/RobotoMono-Bold.ttf"),
    // The Bridge skin: Antonio in two static cuts (600 label, 700 figure).
    include_bytes!("../assets/fonts/AntonioLabel.ttf"),
    include_bytes!("../assets/fonts/AntonioFigure.ttf"),
    // Antonio SemiBold (static instance of the variable font, family "Antonio SB") from google/fonts (OFL).
    include_bytes!("../assets/fonts/Antonio-SemiBold.ttf"),
    // Holo (Rajdhani) and Tactical (B612, B612 Mono), all OFL.
    include_bytes!("../assets/fonts/Rajdhani-Medium.ttf"),
    include_bytes!("../assets/fonts/Rajdhani-SemiBold.ttf"),
    include_bytes!("../assets/fonts/B612-Regular.ttf"),
    include_bytes!("../assets/fonts/B612-Bold.ttf"),
    include_bytes!("../assets/fonts/B612Mono-Regular.ttf"),
    include_bytes!("../assets/fonts/B612Mono-Bold.ttf"),
    // Command (Saira Condensed, Share Tech Mono), both OFL.
    include_bytes!("../assets/fonts/SairaCondensed-Medium.ttf"),
    include_bytes!("../assets/fonts/SairaCondensed-SemiBold.ttf"),
    include_bytes!("../assets/fonts/ShareTechMono-Regular.ttf"),
    // Barlow Semi Condensed: the body font of the Bridge look (OFL).
    include_bytes!("../assets/fonts/BarlowSemiCondensed-Regular.ttf"),
    include_bytes!("../assets/fonts/BarlowSemiCondensed-Medium.ttf"),
    // Operator skin: the VT323 terminal face and a katakana subset of DotGothic16.
    include_bytes!("../assets/fonts/VT323-Regular.ttf"),
    include_bytes!("../assets/fonts/DotGothic16-Katakana.ttf"),
    // Neon skin faces.
    include_bytes!("../assets/fonts/Orbitron-ExtraBold.ttf"),
    include_bytes!("../assets/fonts/ChakraPetch-Medium.ttf"),
    include_bytes!("../assets/fonts/ChakraPetch-Bold.ttf"),
];

/// The asset source of the app. All files are compiled into the binary.
#[derive(Debug)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(svg) = crate::vtext::load(path) {
            return Ok(Some(Cow::Owned(svg)));
        }
        Ok(FILES
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

/// Register IBM Plex Sans and JetBrains Mono with the text system.
pub fn load_fonts(cx: &App) -> Result<()> {
    cx.text_system()
        .add_fonts(FONTS.iter().map(|b| Cow::Borrowed(*b)).collect())
}
