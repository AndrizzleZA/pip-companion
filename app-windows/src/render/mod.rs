//! The desktop half of rendering: fonts loaded from the Windows font folder, the widgets built
//! on them, the colour scheme and the pixel order a layered window wants.
//!
//! The creature itself and the drawing helpers live in `pip-draw`, which has no operating system
//! in it and is shared with the phone (§50). They are re-exported here so every call site in this
//! shell still says `crate::render::…`.

pub use pip_draw::*;

pub mod text;
pub mod ui;

use pip_core::settings::{PetColor, Theme as ThemePref};

/// Windows composites a layered window from BGRA; tiny-skia hands back RGBA.
pub fn rgba_to_bgra(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

/// Resolved colours for one render.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub window: u32,
    pub surface: u32,
    pub surface2: u32,
    pub text: u32,
    pub muted: u32,
    pub border: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub body: u32,
}

impl Palette {
    pub fn new(pref: ThemePref, pet: PetColor) -> Palette {
        let dark = match pref {
            ThemePref::Dark => true,
            ThemePref::Light => false,
            ThemePref::System => crate::platform::system_dark_mode(),
        };
        let accent = pet.accent();
        if dark {
            Palette {
                dark,
                window: 0x1C1F26,
                surface: 0x262A33,
                surface2: 0x323744,
                text: 0xF2F4F8,
                muted: 0x9AA3B2,
                border: 0x3A404D,
                accent: mix(accent, 0xFFFFFF, 0.18),
                on_accent: 0xFFFFFF,
                body: pet.rgb(),
            }
        } else {
            Palette {
                dark,
                window: 0xF6F7F9,
                surface: 0xFFFFFF,
                surface2: 0xEEF0F4,
                text: 0x1E2330,
                muted: 0x6B7385,
                border: 0xE1E4EA,
                accent,
                on_accent: 0xFFFFFF,
                body: pet.rgb(),
            }
        }
    }
}
