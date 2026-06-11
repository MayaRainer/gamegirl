use std::cmp::Ordering;
use sdl2::render::{WindowCanvas};
use sdl2::VideoSubsystem;
use crate::{mmio, System};

pub struct PPU {
	line: u8,
	window_line: u8,
	canvas: WindowCanvas,
	mode: u8,
	wait_cycles: u16,
}

pub const SCALE: usize = 3;
pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;

mod mode {
	pub const HBLANK: u8 = 0;
	pub const VBLANK: u8 = 1;
	pub const OAM_SCAN: u8 = 2;
	pub const DRAWING: u8 = 3;
}

fn load_tile_line(sys: &System, address: u16, y: u8) -> [u8; 8] {
	let tile_address = address + (u16::from(y) % 8) * 2;
	let tile_lo = sys.read(tile_address);
	let tile_hi = sys.read(tile_address + 1);
	std::array::from_fn(|x| ((tile_hi >> (7 - x)) & 1) << 1 | (tile_lo >> (7 - x)) & 1)
}

fn render_bg_window(sys: &mut System, lcd_control: u8, tile_map_bit: u8, y: u8, mut x: u8) -> [u8; SCREEN_WIDTH] {
	let tile_map_offset = if (lcd_control & 1 << tile_map_bit) != 0 { 0x9c00 } else { 0x9800 };
	let mut array = [0u8; 160];
	let mut i = 0u8;
	for _ in 0..(SCREEN_WIDTH / 8) {
		let index = sys.read(tile_map_offset + u16::from(y) / 8 * 32 + u16::from(x) / 8);
		let address = if (lcd_control & 1 << 4) != 0 {
			0x8000 + u16::from(index) * 16
		} else {
			0x9000u16.wrapping_add_signed(i16::from(index.cast_signed()) * 16)
		};
		let line = load_tile_line(sys, address, y);
		loop {
			array[i as usize] = line[(x % 8) as usize];
			i += 1;
			x = x.wrapping_add(1);
			if x.is_multiple_of(8) { break }
		}
	}
	array
}

fn colour_from_palette(palette: u8, id: u8) -> u8 {
	(palette >> (id * 2)) & 0b11
}

impl PPU {
	#[must_use]
	pub fn new(video: &VideoSubsystem) -> Self {
		let window = video.window("GameGirl", (SCREEN_WIDTH * SCALE) as u32, (SCREEN_HEIGHT * SCALE) as u32)
			.position_centered()
			.build()
			.unwrap();
		let mut canvas = window.into_canvas().build().unwrap();
		#[allow(clippy::cast_precision_loss)]
		let scale = SCALE as f32;
		canvas.set_scale(scale, scale).unwrap();
		Self { mode: 2, wait_cycles: 80, line: 0, window_line: u8::MAX, canvas }
	}
	
	fn enter_mode(&mut self, sys: &mut System, mode: u8, wait_cycles: u16) {
		let lcd_status = sys.read(mmio::LCD_STATUS);
		sys.write(mmio::LCD_STATUS, lcd_status & !0b11 | mode);
		if mode < 3 && lcd_status & (1 << (mode + 3)) != 0 {
			sys.interrupt(1 << 1);
		}
		self.mode = mode;
		self.wait_cycles = wait_cycles;
	}
	
	pub(crate) fn step(&mut self, sys: &mut System) {
		let lcd_control = sys.read(mmio::LCD_CONTROL);
		if lcd_control & (1 << 7) == 0 { return }
		if self.wait_cycles > 0 {
			self.wait_cycles -= 1;
			return;
		}
		match self.mode {
			mode::HBLANK | mode::VBLANK => {
				self.line += 1;
				if self.line == 154 {
					self.line = 0;
					self.window_line = u8::MAX;
					self.canvas.present();
				}
				match self.line.cmp(&(SCREEN_HEIGHT as u8)) {
					Ordering::Equal => {
						sys.interrupt(1);
						self.enter_mode(sys, mode::VBLANK, 456);
					}
					Ordering::Less => self.enter_mode(sys, mode::OAM_SCAN, 80),
					Ordering::Greater => self.wait_cycles = 456
				}
				
				let mut lcd_status = sys.read(mmio::LCD_STATUS);
				if self.line == sys.read(mmio::LCD_Y_COMPARE) {
					if lcd_status & (1 << 6) != 0 {
						sys.interrupt(1 << 1);
					}
					lcd_status |= 1 << 2;
				} else {
					lcd_status &= !(1 << 2);
				}
				sys.write(mmio::LCD_STATUS, lcd_status);
				sys.write(mmio::LCD_Y, self.line);
			},
			mode::OAM_SCAN => self.enter_mode(sys, mode::DRAWING, 172),
			mode::DRAWING => {
				let window_y = sys.read(mmio::WINDOW_Y);
				let window_x = sys.read(mmio::WINDOW_X);
				let scy = sys.read(mmio::SCROLL_Y);
				let scx = sys.read(mmio::SCROLL_X);
				let bg = if lcd_control & 1 != 0 {
					let mut bg = render_bg_window(sys, lcd_control, 3, self.line.wrapping_add(scy), scx);
					let x = window_x.saturating_sub(7);
					if lcd_control & (1 << 5) != 0 && self.line >= window_y && x <= (SCREEN_WIDTH as u8) {
						self.window_line = self.window_line.wrapping_add(1);
						let window = render_bg_window(sys, lcd_control, 6, self.window_line, 7u8.saturating_sub(window_x));
						bg[x as usize..].copy_from_slice(&window[..SCREEN_WIDTH - x as usize]);
					}
					bg
				} else { [0u8; 160] };
				
				let mut rendered_objects = [0xffu8; SCREEN_WIDTH];
				if lcd_control & (1 << 1) != 0 {
					let (objects, _) = sys.oam.as_chunks::<4>();
					let obj_palette_1 = sys.read(mmio::OBJ_PALETTE_1);
					let obj_palette_2 = sys.read(mmio::OBJ_PALETTE_2);
					let obj_size = if lcd_control & (1 << 2) != 0 { 16 } else { 8 };
					let mut objects_on_line: Vec<_> = objects.iter().filter(|chunk| {
						(chunk[0]..chunk[0].saturating_add(obj_size)).contains(&(self.line + 16))
					}).collect();
					objects_on_line.sort_by_key(|obj| obj[1]);
					
					for object in objects_on_line.into_iter().take(10) {
						let line_index = self.line + 16 - object[0];
						let line_index = if object[3] & (1 << 6) != 0 { obj_size - line_index - 1 } else { line_index };
						let object_x = usize::from(object[1]);
						let tile_index = u16::from(object[2]);
						
						let tile_index = if obj_size == 16 {
							(tile_index & 0xfe) + u16::from(line_index > 8)
						} else { tile_index };
						let tile_line = load_tile_line(sys, 0x8000 + tile_index * 16, line_index % 8);
						for i in 8usize.saturating_sub(object_x)..(SCREEN_WIDTH + 8).saturating_sub(object_x).min(8) {
							let colour_id = if (object[3] & (1 << 5)) != 0 { tile_line[7 - i] } else { tile_line[i] };
							let x = object_x + i - 8;
							let target = &mut rendered_objects[x];
							if colour_id != 0 && *target == 0xff && (object[3] & (1 << 7) == 0 || bg[x] == 0) {
								*target = colour_from_palette(if object[3] & (1 << 4) != 0 { obj_palette_2 } else { obj_palette_1 }, colour_id);
							}
						}
					}
				}
				
				let bg_palette = sys.read(mmio::BG_PALETTE);
				for x in 0..SCREEN_WIDTH {
					let pixel = if rendered_objects[x] == 0xff {
						colour_from_palette(bg_palette, bg[x])
					} else {
						rendered_objects[x]
					};
					let pixel = (3 - pixel) * 80;
					self.canvas.set_draw_color(sdl2::pixels::Color::RGB(pixel, pixel, pixel));
					self.canvas.draw_point(sdl2::rect::Point::new(i32::from(x as u8), i32::from(self.line))).unwrap();
				}
				self.enter_mode(sys, mode::HBLANK, 204);
			},
			_ => unreachable!(),
		}
	}
}