use macroquad::prelude::*;

#[macroquad::main("BasicShapes")]
async fn main() {
    let url = "ws://192.168.10.12:4000/ws".to_string();
    let mut websocket = quad_net::web_socket::WebSocket::connect(url).unwrap();

    let mut last_r = 0;
    let mut last_g = 0;
    let mut last_b = 0;

    let render_target = render_target(320, 240);
    render_target.texture.set_filter(FilterMode::Nearest);

    loop {
        if websocket.connected() {
            if let Some(msg) = websocket.try_recv() {
                let (t, rest) = msg.split_at(1);
                match t {
                    [b'S'] => {
                        let chunks = rest.split(|x| x == &0xc0);

                        for chunk in chunks {
                            if chunk.is_empty() {
                                continue;
                            }

                            set_camera(&Camera2D {
                                zoom: vec2(0.01, 0.01),
                                target: vec2(0.0, 0.0),
                                render_target: Some(render_target.clone()),
                                ..Default::default()
                            });

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
                                    let mut r = last_r;
                                    let mut g = last_g;
                                    let mut b = last_b;

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

                                    last_r = r;
                                    last_g = g;
                                    last_b = b;

                                    // let rect = Rect::from_x_y_ranges(x..=(x + w), y..=(y + h));
                                    // let shape = Shape::rect_filled(
                                    //     rect,
                                    //     Rounding::ZERO,
                                    //     Color32::from_rgb(r, g, b),
                                    // );

                                    if x == 0.0 && y == 0.0 && w >= 320.0 && h >= 240.0 {
                                        clear_background(BLACK);
                                        // self.rects.clear();
                                    } else {
                                        draw_rectangle(x, y, w, h, Color::from_rgba(r, g, b, 255));
                                    }

                                    // if r == 0 && g == 0 && b == 0 {
                                    // if let Some(idx) = self.rects.iter().enumerate().find_map(
                                    //     |(idx, (old_rect, _old_shape))| {
                                    //         if rect.contains_rect(*old_rect)
                                    //         // if rect.min.x == old_rect.min.x
                                    //         //     && rect.min.y == old_rect.min.y
                                    //         //     && rect.max.x == old_rect.max.x
                                    //         //     && rect.max.y == old_rect.max.y
                                    //         {
                                    //             return Some(idx);
                                    //         }
                                    //         None
                                    //     },
                                    // ) {
                                    //     let _ = self.rects.remove(idx);
                                    // }
                                    // // }
                                    // self.rects.push((rect, shape));
                                }
                                // [0xfd] => {
                                //     let c = frame[0];
                                //     let x = frame[1] as f32 + frame[2] as f32 * 256f32;
                                //     let y = frame[3] as f32 + frame[4] as f32 * 256f32;
                                //     let r = frame[5];
                                //     let g = frame[6];
                                //     let b = frame[7];
                                //
                                //     let rect = painter.text(
                                //         Pos2::new(x, y),
                                //         Align2::CENTER_CENTER,
                                //         c,
                                //         FontId::new(12f32, egui::FontFamily::Monospace),
                                //         Color32::from_rgb(r, g, b),
                                //     );
                                //     let shape =
                                //         Shape::rect_stroke(rect, Rounding::ZERO, Stroke::NONE);
                                //
                                //     if let Some(idx) = self.chars.iter().enumerate().find_map(
                                //         |(idx, (old_rect, _old_shape))| {
                                //             // if rect.min.x == old_rect.min.x
                                //             //     && rect.min.y == old_rect.min.y
                                //             //     && rect.max.x == old_rect.max.x
                                //             //     && rect.max.y == old_rect.max.y
                                //             if rect.contains_rect(*old_rect) {
                                //                 return Some(idx);
                                //             }
                                //             None
                                //         },
                                //     ) {
                                //         let _ = self.chars.remove(idx);
                                //     }
                                //     self.chars.push((rect, shape));
                                // }
                                _ => (),
                            }
                        }
                    }
                    [b'A'] => {}
                    _ => todo!(),
                }

                // if self.rects.len() >= 500 {
                //     self.rects.drain(0..self.rects.len() - 500);
                // }
                // if self.chars.len() >= 500 {
                //     self.chars.drain(0..self.chars.len() - 500);
                // }

                // self.message = Some(event);
            }
        }

        set_default_camera();

        clear_background(BLACK);

        draw_texture_ex(
            &render_target.texture,
            0.,
            0.,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(screen_width(), screen_height())),
                ..Default::default()
            },
        );

        draw_line(40.0, 40.0, 100.0, 200.0, 15.0, BLUE);
        draw_rectangle(screen_width() / 2.0 - 60.0, 100.0, 120.0, 60.0, GREEN);
        draw_circle(screen_width() - 30.0, screen_height() - 30.0, 15.0, YELLOW);

        draw_text("IT WORKS!", 20.0, 20.0, 30.0, DARKGRAY);

        next_frame().await
    }
}
