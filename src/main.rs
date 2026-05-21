#![feature(int_lowest_highest_one)]
#![warn(clippy::pedantic)]
#![allow(
	clippy::too_many_lines,
	clippy::similar_names,
	clippy::cast_possible_truncation,
	clippy::missing_errors_doc,
	clippy::missing_panics_doc
)]


use std::collections::HashSet;
use sdl2::controller::{GameController, self};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use timer::Timer;
use crate::memory::{MemoryMapper};
use crate::timer::TimerReturn;

pub mod cpu;
pub mod memory;
pub mod timer;
pub mod ppu;

mod mmio {
	pub const JOYPAD: u16 = 0xff00;
	pub const TIMER_DIVIDER: u16 = 0xff04;
	pub const TIMER_COUNTER: u16 = 0xff05;
	pub const TIMER_MODULO: u16 = 0xff06;
	pub const TIMER_CONTROL: u16 = 0xff07;
	pub const PENDING_INTERRUPTS: u16 = 0xff0f;
	pub const LCD_CONTROL: u16 = 0xff40;
	pub const LCD_STATUS: u16 = 0xff41;
	pub const SCROLL_Y: u16 = 0xff42;
	pub const SCROLL_X: u16 = 0xff43;
	pub const LCD_Y: u16 = 0xff44;
	pub const LCD_Y_COMPARE: u16 = 0xff45;
	pub const OAM_DMA_TRANSFER: u16 = 0xff46;
	pub const BG_PALETTE: u16 = 0xff47;
	pub const OBJ_PALETTE_1: u16 = 0xff48;
	pub const OBJ_PALETTE_2: u16 = 0xff49;
	pub const WINDOW_Y: u16 = 0xff4a;
	pub const WINDOW_X: u16 = 0xff4b;
	pub const ENABLED_INTERRUPTS: u16 = 0xffff;
}

struct System {
	pressed_keys: HashSet<Keycode>,
	timer: Timer,
	mapper: Box<dyn MemoryMapper>,
	oam: [u8; 0xa0],
	joypad_selection: u8,
	controller: Option<GameController>
}

const JOYSTICK_DEADZONE: i16 = 3000;

impl System {
	fn set_pressed_keys<ControllerPredicate>(&self, keys: &[Keycode], controller_predicate: ControllerPredicate, index: u8) -> u8
	where ControllerPredicate: Fn(&GameController) -> bool {
		if keys.iter().any(|key| self.pressed_keys.contains(key)) || self.controller.as_ref().is_some_and(controller_predicate) {
			!(1 << index)
		} else {
			0xff
		}
	}
	
	pub fn read(&self, address: u16) -> u8 {
		match address {
			mmio::JOYPAD => {
				let mut value = 0b1100_1111 | self.joypad_selection & 0b11_0000;
				if self.joypad_selection & (1 << 4) == 0 {
					value &= self.set_pressed_keys(&[Keycode::RIGHT, Keycode::D], |controller| controller.button(controller::Button::DPadRight) || controller.axis(controller::Axis::LeftX) > JOYSTICK_DEADZONE, 0);
					value &= self.set_pressed_keys(&[Keycode::LEFT, Keycode::A], |controller| controller.button(controller::Button::DPadLeft) || controller.axis(controller::Axis::LeftX) < -JOYSTICK_DEADZONE, 1);
					value &= self.set_pressed_keys(&[Keycode::UP, Keycode::W], |controller| controller.button(controller::Button::DPadUp) || controller.axis(controller::Axis::LeftY) < -JOYSTICK_DEADZONE, 2);
					value &= self.set_pressed_keys(&[Keycode::DOWN, Keycode::S], |controller| controller.button(controller::Button::DPadDown) || controller.axis(controller::Axis::LeftY) > JOYSTICK_DEADZONE, 3);
				}
				if self.joypad_selection & (1 << 5) == 0 {
					value &= self.set_pressed_keys(&[Keycode::SPACE, Keycode::Z], |controller| controller.button(controller::Button::A), 0);
					value &= self.set_pressed_keys(&[Keycode::LCTRL, Keycode::X], |controller| controller.button(controller::Button::B), 1);
					value &= self.set_pressed_keys(&[Keycode::TAB], |controller| controller.button(controller::Button::Guide), 2);
					value &= self.set_pressed_keys(&[Keycode::RETURN], |controller| controller.button(controller::Button::Start), 3);
				}
				value
			},
			0xfe00..0xfea0 => self.oam[address as usize - 0xfe00],
			mmio::TIMER_DIVIDER => self.timer.divider(),
			mmio::TIMER_COUNTER => self.timer.counter,
			mmio::TIMER_MODULO => self.timer.modulo,
			mmio::TIMER_CONTROL => self.timer.control,
			mmio::PENDING_INTERRUPTS => self.mapper.read(address) | 0b1110_0000,
			_ => self.mapper.read(address)
		}
	}
	
	pub fn write(&mut self, address: u16, value: u8) {
		match address {
			0xfe00..0xfea0 => self.oam[address as usize - 0xfe00] = value,
			mmio::JOYPAD => self.joypad_selection = value,
			mmio::OAM_DMA_TRANSFER => {
				let source_address = u16::from(value) * 0x100;
				for i in 0..0xa0 {
					self.oam[usize::from(i)] = self.mapper.read(source_address + i);
				}
			},
			mmio::TIMER_DIVIDER => self.timer.cycle = 0,
			mmio::TIMER_COUNTER => self.timer.counter = value,
			mmio::TIMER_MODULO => self.timer.modulo = value,
			mmio::TIMER_CONTROL => self.timer.control = value,
			address => self.mapper.write(address, value)
		}
	}
	
	pub fn interrupt(&mut self, value: u8) {
		let current = self.read(mmio::PENDING_INTERRUPTS);
		self.write(mmio::PENDING_INTERRUPTS, current | value);
	}
}

fn main() {
	let path = std::env::args().nth(1).expect("No path given");
	let rom = std::fs::read(path).expect("ROM file not found");
	
	let sdl_context = sdl2::init().unwrap();
	let controllers = sdl_context.game_controller().unwrap();
	let num_joysticks = controllers.num_joysticks().unwrap();
	let game_controller = (0..num_joysticks).find(|i| controllers.is_game_controller(*i)).and_then(|i| controllers.open(i).ok());
	let video_subsystem = sdl_context.video().unwrap();
	let window = video_subsystem.window("GameGirl", (ppu::SCREEN_WIDTH * 2) as u32, (ppu::SCREEN_HEIGHT * 2) as u32)
		.position_centered()
		.build()
		.unwrap();
	let mut canvas = window.into_canvas().build().unwrap();
	canvas.set_scale(2.0, 2.0).unwrap();
	
	let mut ppu = ppu::PPU::new(canvas);
	let mut cpu = cpu::CPU::default();
	let mut mapper: Box<dyn MemoryMapper> = match rom[0x147] {
		0 => Box::new(memory::RomOnly { rom, ram: vec![0; 0x8000] }),
		1..=3 => Box::new(memory::MBC1::new(rom)),
		_ => unimplemented!(),
	};
	mapper.write(mmio::JOYPAD, 0b1111);
	let timer = Timer::default();
	let mut sys = System { mapper, timer, oam: [0; 0xa0], pressed_keys: HashSet::new(), joypad_selection: 0, controller: game_controller };
	sys.write(mmio::LCD_CONTROL, 0x91);
	sys.write(mmio::BG_PALETTE, 0xfc);
	
	let mut event_pump = sdl_context.event_pump().unwrap();
	'running: loop {
		let mut cycle = cpu.cycle;
		cpu.step(&mut sys);
		loop {
			if cycle == cpu.cycle {
				break;
			}
			
			for _ in 0..4 {
				ppu.step(&mut sys);
			}
			if sys.timer.step() == TimerReturn::Overflow {
				sys.interrupt(1 << 2);
			}
			
			cycle = cycle.wrapping_add(1);
		}
		for event in event_pump.poll_iter() {
			match event {
				Event::Quit { .. } => break 'running,
				Event::KeyDown { keycode: Some(keycode), .. } => {
					sys.pressed_keys.insert(keycode);
				}
				Event::KeyUp { keycode: Some(keycode), .. } => {
					sys.pressed_keys.remove(&keycode);
				}
				_ => {}
			}
		}
	}
}
