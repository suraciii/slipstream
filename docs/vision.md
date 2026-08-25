# Product Vision

Slipstream is a browser-based photo selection workspace. It lets a Photographer review a shoot quickly from a phone, tablet, or desktop browser, group Photos into Photo Sets, and record keep and reject decisions without modifying Original Files.

## Goal

A Photographer can finish the first selection pass away from a desktop editing application while still seeing a trustworthy representation of each Photo.

Slipstream reduces the cost of deciding what to keep. It does not replace RAW development or final image editing.

## Initial Focus

Slipstream initially serves one Photographer with an existing local or network-mounted Photo Library containing mostly RAW files and some JPEG files.

The first product must support this complete path:

1. The Photographer opens Slipstream in a browser.
2. Slipstream indexes an existing directory without moving or changing Original Files.
3. Slipstream pairs a RAW Original and matching JPEG Original as one Photo when they have the same base name in the same directory.
4. The Photographer creates or opens a Photo Set.
5. The Photographer reviews one Photo at a time.
6. A right swipe selects the Photo and a left swipe rejects it.
7. The Photographer may assign a zero-to-five-star Rating, inspect available Preview detail, and undo a recent decision.
8. Slipstream retains the Photo Set, review progress, Selection State, and Rating.

## Preview Trust

Slipstream must display a camera-produced representation when one is available. It uses a matching JPEG Original first and otherwise uses the RAW Original's largest usable embedded JPEG.

This rule makes the Preview suitable for selection because it preserves the camera's white balance, picture style or film simulation, tone treatment, and orientation as encoded by the camera. Slipstream does not claim that the Preview exposes all recoverable RAW data or matches later output from a RAW editor.

Slipstream must identify the Preview Source. It must not describe an unavailable or low-resolution Preview as a full-resolution RAW rendering.

## Principles

- **Selection first**: Every first-product capability must help the Photographer group, compare, select, reject, rate, or resume Photos.
- **Camera-produced preview**: Prefer the camera's JPEG result over a new generic RAW interpretation.
- **Original ownership**: Original Files remain in place and unchanged.
- **Touch-native review**: Core selection works through direct gestures and also remains accessible through visible controls and keyboard input.
- **Fast continuation**: The Photographer can resume a Review Session without reconstructing prior progress.
- **Focused core**: Slipstream proves Photo Sets, trustworthy Previews, and selection before adding editing or automation.

## What Slipstream Is Not

- Slipstream is not a RAW development application.
- Slipstream is not a color-grading or retouching application.
- Slipstream is not initially a cloud backup service.
- Slipstream is not initially a multi-user digital asset management system.
- Slipstream does not promise parity with Lightroom, Capture One, darktable, RawTherapee, or a camera vendor's desktop software.
- Slipstream does not initially write Selection State or Rating into Original Files or XMP sidecars.
- Slipstream does not initially use AI to select Photos.

## Long-Term Direction

Slipstream may later export selection metadata, support broader Photo Library search, compare similar Photos, or run as a managed personal service. These directions must preserve the initial ownership and Preview Trust boundaries.
