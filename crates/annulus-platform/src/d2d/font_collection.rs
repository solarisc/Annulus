//! Private DirectWrite font collections and Nerd symbol fallback, without OS installation.

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_UNICODE_RANGE, IDWriteFactory, IDWriteFactory3, IDWriteFactory5, IDWriteFontCollection,
    IDWriteFontFallback, IDWriteInMemoryFontFileLoader,
};
use windows::core::{HSTRING, Interface, PCWSTR};

struct RegisteredLoader {
    factory: IDWriteFactory,
    loader: IDWriteInMemoryFontFileLoader,
}

impl Drop for RegisteredLoader {
    fn drop(&mut self) {
        // SAFETY: this wrapper registered the loader; collection and fallback references
        // have been dropped before the wrapper is dropped.
        unsafe {
            let _ = self.factory.UnregisterFontFileLoader(&self.loader);
        }
    }
}

/// A collection backed by copies owned by DirectWrite, and its fallback mapping.
pub(super) struct FontCollection {
    pub collection: IDWriteFontCollection,
    pub fallback: IDWriteFontFallback,
    pub revision: u64,
    _loader: RegisteredLoader,
}

impl FontCollection {
    pub fn new(factory: &IDWriteFactory) -> windows::core::Result<Self> {
        let (revision, data) = crate::fonts::snapshot();
        let modern: IDWriteFactory5 = factory.cast()?;
        // SAFETY: the loader and factory are used on this renderer's owning thread.
        let loader = unsafe {
            let loader = modern.CreateInMemoryFontFileLoader()?;
            factory.RegisterFontFileLoader(&loader)?;
            RegisteredLoader {
                factory: factory.clone(),
                loader,
            }
        };
        // SAFETY: null ownerObject requests a DirectWrite-owned COPY of each byte buffer.
        // All pointers and slices outlive their calls; the loader remains registered for
        // the lifetime of the collection and its fallback mappings.
        let (collection, fallback) = unsafe {
            let builder = modern.CreateFontSetBuilder()?;
            for bytes in data {
                let Ok(size) = u32::try_from(bytes.len()) else {
                    continue;
                };
                let file = loader.loader.CreateInMemoryFontFileReference(
                    factory,
                    bytes.as_ref().as_ptr().cast(),
                    size,
                    None,
                )?;
                builder.AddFontFile(&file)?;
            }
            let set = builder.CreateFontSet()?;
            let base: IDWriteFactory3 = factory.cast()?;
            let collection: IDWriteFontCollection =
                base.CreateFontCollectionFromFontSet(&set)?.cast()?;
            let fallback = modern.CreateFontFallbackBuilder()?;
            let symbols = HSTRING::from(crate::fonts::SYMBOL_FAMILY);
            fallback.AddMapping(
                &[
                    DWRITE_UNICODE_RANGE {
                        first: 0xE000,
                        last: 0xF8FF,
                    },
                    DWRITE_UNICODE_RANGE {
                        first: 0xF0000,
                        last: 0xFFFFD,
                    },
                    DWRITE_UNICODE_RANGE {
                        first: 0x0010_0000,
                        last: 0x0010_FFFD,
                    },
                ],
                &[symbols.as_ptr()],
                &collection,
                PCWSTR::null(),
                PCWSTR::null(),
                1.0,
            )?;
            fallback.AddMappings(&modern.GetSystemFontFallback()?)?;
            (collection, fallback.CreateFontFallback()?)
        };
        Ok(Self {
            collection,
            fallback,
            revision,
            _loader: loader,
        })
    }

    pub fn contains(&self, name: &HSTRING) -> bool {
        let mut index = 0;
        let mut exists = windows::core::BOOL::default();
        // SAFETY: both output pointers and the terminated family string outlive the call.
        unsafe {
            self.collection
                .FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists)
                .is_ok()
                && exists.as_bool()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    };

    #[test]
    fn directwrite_has_real_bmp_and_supplementary_nerd_glyphs_without_os_installation() {
        let d2d = super::super::D2d::new().unwrap();
        let fonts = d2d.fonts.borrow();
        let name = HSTRING::from(crate::fonts::SYMBOL_FAMILY);
        let mut index = 0;
        let mut exists = windows::core::BOOL::default();
        let codepoints = [0xE0B0, 0xF120, 0xF0001];
        let mut glyphs = [0; 3];
        // SAFETY: output variables and buffers outlive these synchronous COM calls; all
        // interfaces stay on the renderer's owning thread and die before its loader.
        unsafe {
            fonts
                .collection
                .FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists)
                .unwrap();
            assert!(exists.as_bool());
            let family = fonts.collection.GetFontFamily(index).unwrap();
            let font = family
                .GetFirstMatchingFont(
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                )
                .unwrap();
            let face = font.CreateFontFace().unwrap();
            face.GetGlyphIndices(codepoints.as_ptr(), 3, glyphs.as_mut_ptr())
                .unwrap();
        }
        assert!(glyphs.iter().all(|id| *id != 0), "{glyphs:?}");
    }
}
