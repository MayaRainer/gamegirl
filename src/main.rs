#![feature(int_lowest_highest_one)]
#![feature(thread_sleep_until)]
#![feature(integer_extend_truncate)]
#![warn(clippy::pedantic)]
#![allow(
	clippy::too_many_lines,
	clippy::similar_names,
	clippy::cast_possible_truncation,
	clippy::missing_errors_doc,
	clippy::missing_panics_doc,
	clippy::struct_excessive_bools,
)]


use std::collections::HashSet;
use std::ops::Add;
use std::time::{Duration, Instant};
use sdl2::controller::{GameController, self};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use timer::Timer;
use crate::audio::Audio;
use crate::memory::{MemoryMapper};
use crate::timer::TimerReturn;

pub mod cpu;
pub mod memory;
pub mod timer;
pub mod ppu;
mod audio;

mod mmio {
	pub const JOYPAD: u16 = 0xff00;
	pub const TIMER_DIVIDER: u16 = 0xff04;
	pub const TIMER_COUNTER: u16 = 0xff05;
	pub const TIMER_MODULO: u16 = 0xff06;
	pub const TIMER_CONTROL: u16 = 0xff07;
	pub const PENDING_INTERRUPTS: u16 = 0xff0f;
	pub const AUDIO_VOLUME: u16 = 0xff24;
	pub const AUDIO_PANNING: u16 = 0xff25;
	pub const AUDIO_CONTROL: u16 = 0xff26;
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
	memory: Box<dyn MemoryMapper>,
	oam: [u8; 0xa0],
	joypad_selection: u8,
	controller: Option<GameController>,
	audio: Audio,
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
			mmio::PENDING_INTERRUPTS => self.memory.read(address) | 0b1110_0000,
			0xff10..0xff40 => self.audio.read_register(address),
			_ => self.memory.read(address)
		}
	}
	
	pub fn write(&mut self, address: u16, value: u8) {
		match address {
			0xfe00..0xfea0 => self.oam[address as usize - 0xfe00] = value,
			mmio::JOYPAD => self.joypad_selection = value,
			mmio::OAM_DMA_TRANSFER => {
				let source_address = u16::from(value) * 0x100;
				for i in 0..0xa0 {
					self.oam[usize::from(i)] = self.memory.read(source_address + i);
				}
			},
			mmio::TIMER_DIVIDER => self.timer.cycle = 0,
			mmio::TIMER_COUNTER => self.timer.counter = value,
			mmio::TIMER_MODULO => self.timer.modulo = value,
			mmio::TIMER_CONTROL => self.timer.control = value,
			0xff10..0xff40 => self.audio.write_register(address, value),
			address => self.memory.write(address, value)
		}
	}
	
	pub fn interrupt(&mut self, value: u8) {
		let current = self.read(mmio::PENDING_INTERRUPTS);
		self.write(mmio::PENDING_INTERRUPTS, current | value);
	}
}

const SYNC_CYCLES: u16 = 2u16.pow(14);
const CYCLES_PER_SECOND: u32 = 4_194_304 / 4;
const SYNC_CYCLE_DURATION: u32 = (Duration::from_secs(1).as_nanos() as u32) / (CYCLES_PER_SECOND / SYNC_CYCLES as u32);

fn main() {
	let path = std::env::args().nth(1).expect("No path given");
	let rom = std::fs::read(path).expect("ROM file not found");
	
	let sdl_context = sdl2::init().unwrap();
	let controllers = sdl_context.game_controller().unwrap();
	let num_joysticks = controllers.num_joysticks().unwrap();
	let controller = (0..num_joysticks).find(|i| controllers.is_game_controller(*i)).and_then(|i| controllers.open(i).ok());
	
	let mut ppu = ppu::PPU::new(&sdl_context.video().unwrap());
	let mut cpu = cpu::CPU::default();
	let mut memory: Box<dyn MemoryMapper> = match rom[0x147] {
		0 => Box::new(memory::RomOnly::new(rom)),
		1..=3 => Box::new(memory::MBC1::new(rom)),
		0x11..=0x13 => Box::new(memory::MBC3::new(rom)),
		_ => unimplemented!(),
	};
	memory.write(mmio::JOYPAD, 0b1111);
	let timer = Timer::default();
	
	let audio = Audio::new(&sdl_context.audio().unwrap());
	
	let mut sys = System { memory, timer, oam: [0; 0xa0], pressed_keys: HashSet::new(), joypad_selection: 0, controller, audio };
	sys.write(mmio::LCD_CONTROL, 0x91);
	sys.write(mmio::BG_PALETTE, 0xfc);
	
	let mut event_pump = sdl_context.event_pump().unwrap();
	let mut time = Instant::now();
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
			sys.audio.step();
			
			if cycle.is_multiple_of(SYNC_CYCLES) {
				for event in event_pump.poll_iter() {
					match event {
						Event::Quit { .. } => {
							sys.memory.save();
							break 'running
						},
						Event::KeyDown { keycode: Some(keycode), .. } => {
							sys.pressed_keys.insert(keycode);
						}
						Event::KeyUp { keycode: Some(keycode), .. } => {
							sys.pressed_keys.remove(&keycode);
						}
						_ => {}
					}
				}
				let sleep_until = time.add(Duration::new(0, SYNC_CYCLE_DURATION));
				assert!(!sleep_until.duration_since(Instant::now()).is_zero(), "Lag");
				std::thread::sleep_until(sleep_until);
				time = sleep_until;
			}
			
			cycle = cycle.wrapping_add(1);
		}
	}
}
