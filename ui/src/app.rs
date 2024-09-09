use std::{io::BufRead, time::Duration};

use egui::{
    epaint::TextShape, Align2, Color32, FontId, Painter, Pos2, Rect, Rounding, Shape, Stroke,
};
use ewebsock::{WsReceiver, WsSender};
use log::{debug, info};

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
// #[derive(serde::Deserialize, serde::Serialize)]
// #[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct RemoteM8UI {
    // Example stuff:
    label: String,

    // #[serde(skip)] // This how you opt-out of serialization of a field
    value: f32,
    message: Option<Vec<u8>>,
    // #[serde(skip)] // This how you opt-out of serialization of a field
    ws_sender: WsSender,
    // #[serde(skip)] // This how you opt-out of serialization of a field
    ws_receiver: WsReceiver,
    rects: Vec<(Rect, Shape)>,
    last_r: u8,
    last_g: u8,
    last_b: u8,
    chars: Vec<(Rect, Shape)>,
}

// impl Default for RemoteM8UI {
//     fn default() -> Self {
//         Self {
//             // Example stuff:
//             label: "Hello World!".to_owned(),
//             value: 2.7,
//         }
//     }
// }

impl RemoteM8UI {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        // if let Some(storage) = cc.storage {
        //     return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        // }
        let ctx = cc.egui_ctx.clone();

        let wakeup = move || ctx.request_repaint(); // wake up UI thread on new message
                                                    // let url = "ws://localhost:4000/ws";
        let url = "ws://192.168.10.12:4000/ws";
        let (ws_sender, ws_receiver) =
            match ewebsock::connect_with_wakeup(url, Default::default(), wakeup) {
                Ok((ws_sender, ws_receiver)) => {
                    // self.frontend = Some(FrontEnd::new(ws_sender, ws_receiver));
                    // self.error.clear();
                    (ws_sender, ws_receiver)
                }
                Err(error) => {
                    log::error!("Failed to connect to {:?}: {}", url, error);
                    panic!("Couldn't connect");
                }
            };

        // Default::default()
        Self {
            label: "Label".to_string(),
            value: 2.0,
            message: None,
            ws_sender,
            ws_receiver,
            rects: Vec::new(),
            last_r: 0,
            last_g: 0,
            last_b: 0,
            chars: Vec::new(),
        }
    }
}

impl eframe::App for RemoteM8UI {
    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        // eframe::set_value(storage, eframe::APP_KEY, self);
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Put your widgets into a `SidePanel`, `TopBottomPanel`, `CentralPanel`, `Window` or `Area`.
        // For inspiration and more examples, go to https://emilk.github.io/egui

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:

            egui::menu::bar(ui, |ui| {
                // NOTE: no File->Quit on web pages!
                let is_web = cfg!(target_arch = "wasm32");
                if !is_web {
                    ui.menu_button("File", |ui| {
                        if ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                    ui.add_space(16.0);
                }

                egui::widgets::global_dark_light_mode_buttons(ui);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // The central panel the region left after adding TopPanel's and SidePanel's
            ui.heading("eframe template");

            let (response, painter) = ui.allocate_painter(
                egui::Vec2::new(ui.available_width(), 300.0),
                egui::Sense::hover(),
            );

            // painter.add(egui::Shape::rect_filled(
            //     egui::Rect::from_two_pos(
            //         egui::Pos2::new(10.0, 10.0),
            //         egui::Pos2::new(200.0, 200.0),
            //     ),
            //     egui::Rounding::ZERO,
            //     egui::Color32::RED,
            // ));
            for (_rect, shape) in self.rects.iter() {
                painter.add(shape.clone());
            }
            for (_rect, shape) in self.chars.iter() {
                painter.add(shape.clone());
            }

            egui::Frame::canvas(ui.style()).show(ui, |_ui| response);

            ui.horizontal(|ui| {
                ui.label("Write something: ");
                ui.text_edit_singleline(&mut self.label);
            });

            ui.add(egui::Slider::new(&mut self.value, 0.0..=10.0).text("value"));
            if ui.button("Increment").clicked() {
                self.value += 1.0;
            }

            ui.separator();

            // if let Some(event) = &self.message {
            //     // ui.label(format!("{:02X?}", event));
            // }

            ui.separator();

            ui.add(egui::github_link_file!(
                "https://github.com/emilk/eframe_template/blob/main/",
                "Source code."
            ));

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                powered_by_egui_and_eframe(ui);
                egui::warn_if_debug_build(ui);
            });

            while let Some(event) = self.ws_receiver.try_recv() {
                match event {
                    ewebsock::WsEvent::Opened => (),
                    ewebsock::WsEvent::Message(ewebsock::WsMessage::Binary(msg)) => {
                        // self.message = Some(msg)
                        let (t, rest) = msg.split_at(1);
                        match t {
                            [b'S'] => {
                                let chunks = rest.split(|x| x == &0xc0);

                                for chunk in chunks {
                                    if chunk.is_empty() {
                                        continue;
                                    }
                                    let mut tmp = vec![0xc0, 0xc0];
                                    tmp.extend_from_slice(chunk);
                                    tmp.push(0xc0);
                                    let decoded = simple_slip::decode(&tmp).unwrap();

                                    let (t, frame) = decoded.split_at(1);

                                    match t {
                                        [0xfe] => {
                                            let x = frame[0] as f32 + frame[1] as f32 * 256f32;
                                            let y = frame[2] as f32 + frame[3] as f32 * 256f32;
                                            let mut w = 1.0f32;
                                            let mut h = 1.0f32;
                                            let mut r = self.last_r;
                                            let mut g = self.last_g;
                                            let mut b = self.last_b;

                                            match frame.len() {
                                                11 => {
                                                    w = frame[4] as f32 + frame[5] as f32 * 256f32;
                                                    h = frame[6] as f32 + frame[7] as f32 * 256f32;
                                                    r = frame[8];
                                                    g = frame[9];
                                                    b = frame[10];
                                                }
                                                8 => {
                                                    w = frame[4] as f32 + frame[5] as f32 * 256f32;
                                                    h = frame[6] as f32 + frame[7] as f32 * 256f32;
                                                }
                                                7 => {
                                                    r = frame[4];
                                                    g = frame[5];
                                                    b = frame[6];
                                                }
                                                5 => {
                                                    w = 1f32;
                                                    h = 1f32;
                                                }
                                                _ => (),
                                            }

                                            self.last_r = r;
                                            self.last_g = g;
                                            self.last_b = b;

                                            let rect =
                                                Rect::from_x_y_ranges(x..=(x + w), y..=(y + h));
                                            let shape = Shape::rect_filled(
                                                rect,
                                                Rounding::ZERO,
                                                Color32::from_rgb(r, g, b),
                                            );

                                            if x == 0.0 && y == 0.0 && w >= 320.0 && h >= 240.0 {
                                                self.rects.clear();
                                            }
                                            // if r == 0 && g == 0 && b == 0 {
                                            if let Some(idx) =
                                                self.rects.iter().enumerate().find_map(
                                                    |(idx, (old_rect, _old_shape))| {
                                                        if rect.contains_rect(*old_rect)
                                                        // if rect.min.x == old_rect.min.x
                                                        //     && rect.min.y == old_rect.min.y
                                                        //     && rect.max.x == old_rect.max.x
                                                        //     && rect.max.y == old_rect.max.y
                                                        {
                                                            return Some(idx);
                                                        }
                                                        None
                                                    },
                                                )
                                            {
                                                let _ = self.rects.remove(idx);
                                            }
                                            // }
                                            self.rects.push((rect, shape));
                                        }
                                        [0xfd] => {
                                            let c = frame[0];
                                            let x = frame[1] as f32 + frame[2] as f32 * 256f32;
                                            let y = frame[3] as f32 + frame[4] as f32 * 256f32;
                                            let r = frame[5];
                                            let g = frame[6];
                                            let b = frame[7];

                                            let rect = painter.text(
                                                Pos2::new(x, y),
                                                Align2::CENTER_CENTER,
                                                c,
                                                FontId::new(12f32, egui::FontFamily::Monospace),
                                                Color32::from_rgb(r, g, b),
                                            );
                                            let shape = Shape::rect_stroke(
                                                rect,
                                                Rounding::ZERO,
                                                Stroke::NONE,
                                            );

                                            if let Some(idx) =
                                                self.chars.iter().enumerate().find_map(
                                                    |(idx, (old_rect, _old_shape))| {
                                                        // if rect.min.x == old_rect.min.x
                                                        //     && rect.min.y == old_rect.min.y
                                                        //     && rect.max.x == old_rect.max.x
                                                        //     && rect.max.y == old_rect.max.y
                                                        if rect.contains_rect(*old_rect) {
                                                            return Some(idx);
                                                        }
                                                        None
                                                    },
                                                )
                                            {
                                                let _ = self.chars.remove(idx);
                                            }
                                            self.chars.push((rect, shape));
                                        }
                                        _ => (),
                                    }
                                }
                            }
                            [b'A'] => {}
                            _ => todo!(),
                        }

                        if self.rects.len() >= 500 {
                            self.rects.drain(0..self.rects.len() - 500);
                        }
                        if self.chars.len() >= 500 {
                            self.chars.drain(0..self.chars.len() - 500);
                        }
                    }
                    ewebsock::WsEvent::Message(ewebsock::WsMessage::Text(_)) => todo!(),
                    ewebsock::WsEvent::Message(ewebsock::WsMessage::Unknown(_)) => todo!(),
                    ewebsock::WsEvent::Message(ewebsock::WsMessage::Ping(_)) => todo!(),
                    ewebsock::WsEvent::Message(ewebsock::WsMessage::Pong(_)) => todo!(),
                    ewebsock::WsEvent::Error(_) => todo!(),
                    ewebsock::WsEvent::Closed => todo!(),
                }

                // self.message = Some(event);
            }
        });
    }
}

fn powered_by_egui_and_eframe(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label("Powered by ");
        ui.hyperlink_to("egui", "https://github.com/emilk/egui");
        ui.label(" and ");
        ui.hyperlink_to(
            "eframe",
            "https://github.com/emilk/egui/tree/master/crates/eframe",
        );
        ui.label(".");
    });
}
