//! Rasterize the actual egui frame without opening/capturing a desktop window.
use egui::{
    Color32,
    epaint::{ImageData, Primitive},
};
use std::{collections::HashMap, path::Path};

pub fn render(
    path: &Path,
    tab: super::PackageTab,
    prepared: bool,
    language: Option<super::UiLanguage>,
    menu_open: bool,
    details_open: bool,
    update_available: bool,
) -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut app = super::App::new(&ctx, true);
    if prepared {
        app.settings = super::Settings::load();
        app.accept_discovery(super::GameDiscovery {
            requested_folder: app.settings.game_folder.clone(),
            installations: super::game_discovery::discover(),
        });
        let startup = super::Startup::load(&app.settings, &super::engine::data_dir(), &|_| {});
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(super::Event::Done(Ok(super::Work::Startup(startup))))
            .map_err(|e| e.to_string())?;
        app.receiver = Some(receiver);
    }
    app.tab = tab;
    if tab == super::PackageTab::Mods {
        app.settings.game_folder = "C:\\Games\\Paladins".into();
        app.mods_folder = app.settings.game_folder.clone();
        app.mods = Ok(vec![
            super::mods::ModInfo {
                id: "example-mod".into(),
                name: "Example Tempest mod".into(),
                version: "1.0.0".into(),
                author: "Example author".into(),
                kind: "V2".into(),
                enabled: true,
            },
            super::mods::ModInfo {
                id: "disabled-example".into(),
                name: "Disabled mod".into(),
                version: "1.0.0".into(),
                author: "Example author".into(),
                kind: "V2".into(),
                enabled: false,
            },
        ]);
    }
    if let Some(language) = language {
        app.settings.ui_language = language;
    }
    if menu_open {
        egui::Popup::open_id(&ctx, super::language_menu::popup_id());
    }
    if update_available {
        app.launcher_update = super::LauncherUpdate::Checked(super::self_update::Check::Available(
            super::self_update::Release {
                version: "0.3.9".into(),
                sha256: "a".repeat(64),
                bytes: 1024,
            },
        ));
    }
    if details_open {
        app.preview_repository_open = true;
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(super::Event::Done(Ok(super::Work::Launch(super::launch::LaunchResult {
                launcher_pid: 36288,
                game_pid: Some(36288),
                log: super::engine::data_dir().join("launch-logs/launch-preview.log"),
                status: "Game dispatched (PID 36288); fresh selected game observed (PID 36288). Mod initialization is not yet verified.".into(),
            }))))
            .map_err(|e| e.to_string())?;
        app.receiver = Some(receiver);
    }
    // Review at the initial window size, with primary package controls visible.
    let size = super::DEFAULT_WINDOW_SIZE.map(|dimension| dimension as usize);
    let mut textures: HashMap<egui::TextureId, egui::ColorImage> = HashMap::new();
    let mut output = None;
    // Fonts/layout may request a second pass; retain texture updates across passes.
    for pass in 0..3 {
        let mut frame = ctx.run_ui(
            egui::RawInput {
                // Settle the popup's ordinary fade animation in CPU previews.
                time: Some(pass as f64 * 0.25),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0] as f32, size[1] as f32),
                )),
                ..Default::default()
            },
            |ui| app.render(ui),
        );
        for (id, deltas) in &frame.textures_delta.set {
            for delta in deltas {
                let ImageData::Color(image) = &delta.image;
                let image = (**image).clone();
                if let Some([x, y]) = delta.pos {
                    let destination = textures.get_mut(id).ok_or("Missing partial texture")?;
                    for row in 0..image.size[1] {
                        for column in 0..image.size[0] {
                            destination.pixels[(y + row) * destination.size[0] + x + column] =
                                image.pixels[row * image.size[0] + column];
                        }
                    }
                } else {
                    textures.insert(*id, image);
                }
            }
        }
        // Texture deltas were applied to the CPU texture store above.
        frame.textures_delta.clear();
        output = Some(frame);
    }
    let output = output.unwrap();
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    let mut pixels = vec![super::BG; size[0] * size[1]];
    for primitive in primitives {
        let Primitive::Mesh(mesh) = primitive.primitive else {
            continue;
        };
        let Some(texture) = textures.get(&mesh.texture_id) else {
            continue;
        };
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let a = mesh.vertices[triangle[0] as usize];
            let b = mesh.vertices[triangle[1] as usize];
            let c = mesh.vertices[triangle[2] as usize];
            let min_x = a
                .pos
                .x
                .min(b.pos.x)
                .min(c.pos.x)
                .max(primitive.clip_rect.min.x)
                .max(0.)
                .floor() as usize;
            let max_x = a
                .pos
                .x
                .max(b.pos.x)
                .max(c.pos.x)
                .min(primitive.clip_rect.max.x)
                .min(size[0] as f32)
                .ceil() as usize;
            let min_y = a
                .pos
                .y
                .min(b.pos.y)
                .min(c.pos.y)
                .max(primitive.clip_rect.min.y)
                .max(0.)
                .floor() as usize;
            let max_y = a
                .pos
                .y
                .max(b.pos.y)
                .max(c.pos.y)
                .min(primitive.clip_rect.max.y)
                .min(size[1] as f32)
                .ceil() as usize;
            let denominator = (b.pos.y - c.pos.y) * (a.pos.x - c.pos.x)
                + (c.pos.x - b.pos.x) * (a.pos.y - c.pos.y);
            if denominator.abs() < 0.0001 {
                continue;
            }
            for y in min_y..max_y {
                for x in min_x..max_x {
                    let px = x as f32 + 0.5;
                    let py = y as f32 + 0.5;
                    let wa = ((b.pos.y - c.pos.y) * (px - c.pos.x)
                        + (c.pos.x - b.pos.x) * (py - c.pos.y))
                        / denominator;
                    let wb = ((c.pos.y - a.pos.y) * (px - c.pos.x)
                        + (a.pos.x - c.pos.x) * (py - c.pos.y))
                        / denominator;
                    let wc = 1. - wa - wb;
                    if wa < 0. || wb < 0. || wc < 0. {
                        continue;
                    }
                    let u = wa * a.uv.x + wb * b.uv.x + wc * c.uv.x;
                    let v = wa * a.uv.y + wb * b.uv.y + wc * c.uv.y;
                    let tx = ((u * texture.size[0] as f32) as usize).min(texture.size[0] - 1);
                    let ty = ((v * texture.size[1] as f32) as usize).min(texture.size[1] - 1);
                    let texel = texture.pixels[ty * texture.size[0] + tx].to_array();
                    let ca = a.color.to_array();
                    let cb = b.color.to_array();
                    let cc = c.color.to_array();
                    let mut source = [0.; 4];
                    for channel in 0..4 {
                        source[channel] = (wa * ca[channel] as f32
                            + wb * cb[channel] as f32
                            + wc * cc[channel] as f32)
                            * texel[channel] as f32
                            / 255.;
                    }
                    let before = pixels[y * size[0] + x].to_array();
                    pixels[y * size[0] + x] = Color32::from_rgb(
                        (source[0] + before[0] as f32 * (1. - source[3] / 255.)).min(255.) as u8,
                        (source[1] + before[1] as f32 * (1. - source[3] / 255.)).min(255.) as u8,
                        (source[2] + before[2] as f32 * (1. - source[3] / 255.)).min(255.) as u8,
                    );
                }
            }
        }
    }
    let bytes: Vec<_> = pixels
        .into_iter()
        .flat_map(|color| color.to_array())
        .collect();
    image::save_buffer(
        path,
        &bytes,
        size[0] as u32,
        size[1] as u32,
        image::ColorType::Rgba8,
    )
    .map_err(|e| e.to_string())
}
