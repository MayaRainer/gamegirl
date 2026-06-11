use sdl2::audio::{AudioQueue};
use crate::mmio;

#[derive(Default)]
struct ChannelVolume {
	register: u8,
	counter: u8,
	value: i8,
}

impl ChannelVolume {
	fn envelope(&mut self, cycle: u16) {
		let envelope_pace = self.register & 0b111;
		if cycle.is_multiple_of(0x4000) && envelope_pace != 0 {
			self.counter += 1;
			if self.counter >= envelope_pace {
				self.counter = 0;
				if self.register & (1 << 3) != 0 {
					self.value = (self.value + 1).min(16);
				} else {
					self.value = (self.value - 1).max(0);
				}
			}
		}
	}
	
	fn set_initial(&mut self) {
		self.value = (self.register >> 4).cast_signed();
	}
}

#[derive(Default)]
struct ChannelStatus<const TIMER_MASK: u8> {
	enabled: bool,
	timer: u8,
	timer_enabled: bool,
	timer_expired: bool,
	dac_enabled: bool,
}

impl<const TIMER_MASK: u8> ChannelStatus<TIMER_MASK> {
	fn step(&mut self, cycle: u16) {
		if cycle.is_multiple_of(0x1000) && self.timer_enabled {
			if self.timer == 0 {
				self.enabled = false;
				self.timer_expired = true;
			} else {
				self.timer -= 1;
			}
		}
	}
	
	fn set_dac_enabled(&mut self, enabled: bool) {
		self.dac_enabled = enabled;
		if !enabled {
			self.enabled = false;
		}
	}
	
	fn set_timer(&mut self, value: u8) {
		self.timer = !value & TIMER_MASK;
		self.timer_expired = false;
	}
	
	fn read_control_register(&self) -> u8 {
		0b1011_1111 | (u8::from(self.timer_enabled) << 6)
	}
	
	fn trigger(&mut self, control: u8) -> bool {
		self.timer_enabled = control & (1 << 6) != 0;
		if !self.dac_enabled { return false; }
		if control & (1 << 7) != 0 {
			self.enabled = true;
			if self.timer_expired {
				self.set_timer(0);
			}
			return true;
		}
		false
	}
}

#[derive(Default, Clone)]
struct Samples {
	phase: f32,
	sample_rate: f32,
}

impl Samples {
	fn samples_to_generate(&mut self, out_rate: f32) -> u8 {
		self.phase += self.sample_rate / out_rate;
		#[allow(clippy::cast_sign_loss)]
		let value = self.phase as u8;
		self.phase %= 1.0;
		value
	}
}

trait Channel {
	fn step(&mut self, cycle: u16);
	fn next(&mut self, sample_rate: f32) -> i8;
}

#[derive(Default)]
pub struct SquareWave {
	period: u16,
	status: ChannelStatus<0b11_1111>,
	volume: ChannelVolume,
	sweep: u8,
	sweep_counter: u8,
	duty: u8,
	samples: Samples,
	sample_counter: u8,
}

impl SquareWave {
	const WAVES: [[i8; 8]; 4] = [
		[-1, 1, 1, 1, 1, 1, 1, 1],
		[1, -1, -1, -1, -1, -1, -1, 1],
		[1, -1, -1, -1, -1, 1, 1, 1],
		[-1, 1, 1, 1, 1, 1, 1, -1],
	];
	
	fn set_period(&mut self, period: u16) {
		self.period = period;
		self.samples.sample_rate = 1_048_576_f32 / (2048f32 - f32::from(period));
	}
	
	fn get_sweep_step(&self) -> u8 {
		self.sweep & 0b111
	}
	
	fn get_sweep_pace(&self) -> u8 {
		(self.sweep >> 4) & 0b111
	}
	
	fn step_sweep(&mut self) {
		let pace = self.get_sweep_pace();
		self.sweep_counter += 1;
		if self.sweep_counter >= pace {
			self.sweep_counter = 0;
			let change = self.period >> self.get_sweep_step();
			let period = if self.sweep & (1 << 3) != 0 {
				self.period - change
			} else {
				self.period + change
			};
			if period + change > 0x7ff {
				self.status.enabled = false;
			} else {
				self.set_period(period);
			}
		}
	}
	
	fn write_duty_register(&mut self, value: u8) {
		self.duty = value;
		self.status.set_timer(value);
	}
	
	fn write_volume_register(&mut self, volume: u8) {
		self.volume.register = volume;
		self.status.set_dac_enabled((volume >> 3) != 0);
	}
	
	fn write_control_register(&mut self, control: u8) {
		self.set_period(self.period & 0xff | ((u16::from(control) & 0b111) << 8));
		if self.status.trigger(control) {
			self.volume.set_initial();
			if self.get_sweep_step() != 0 {
				self.step_sweep();
			}
		}
	}
	
	fn read_register(&self, offset: u16) -> u8 {
		match offset {
			0 => self.sweep | (1 << 7),
			1 => self.duty | 0b11_1111,
			2 => self.volume.register,
			3 => 0xff,
			4 => self.status.read_control_register(),
			_ => unreachable!()
		}
	}
	
	fn write_register(&mut self, offset: u16, value: u8) {
		match offset {
			0 => self.sweep = value,
			1 => self.write_duty_register(value),
			2 => self.write_volume_register(value),
			3 => self.set_period(self.period & 0xff00 | u16::from(value)),
			4 => self.write_control_register(value),
			_ => unreachable!()
		}
	}
}

impl Channel for SquareWave {
	fn step(&mut self, cycle: u16) {
		self.status.step(cycle);
		self.volume.envelope(cycle);
		
		if cycle.is_multiple_of(0x2000) && self.get_sweep_pace() != 0 && self.status.enabled {
			self.step_sweep();
		}
	}
	
	fn next(&mut self, out_rate: f32) -> i8 {
		if self.status.enabled {
			let wave = Self::WAVES[(self.duty >> 6) as usize];
			self.sample_counter = (self.sample_counter + self.samples.samples_to_generate(out_rate)) % 8;
			self.volume.value * wave[self.sample_counter as usize]
		} else {
			0
		}
	}
}

#[derive(Default)]
pub struct Wave {
	period: u16,
	status: ChannelStatus<0xff>,
	volume: u8,
	samples: Samples,
	pattern: [u8; 16],
	sample_counter: u8,
}

impl Wave {
	fn set_period(&mut self, period: u16) {
		self.period = period;
		self.samples.sample_rate = 2_097_152_f32 / (2048f32 - f32::from(period));
	}
	
	fn write_control_register(&mut self, control: u8) {
		self.set_period(self.period & 0xff | ((u16::from(control) & 0b111) << 8));
		self.status.trigger(control);
	}
	
	fn read_register(&self, offset: u16) -> u8 {
		match offset {
			0 => 0b111_1111 | (u8::from(self.status.dac_enabled) << 7),
			1 | 3 => 0xff,
			2 => 0b1001_1111 | (self.volume << 5),
			4 => self.status.read_control_register(),
			_ => unreachable!()
		}
	}
	
	fn write_register(&mut self, offset: u16, value: u8) {
		match offset {
			0 => self.status.set_dac_enabled(value & (1 << 7) != 0),
			1 => self.status.set_timer(value),
			2 => self.volume = (value >> 5) & 0b11,
			3 => self.set_period(self.period & 0xff00 | u16::from(value)),
			4 => self.write_control_register(value),
			_ => unreachable!()
		}
	}
}

impl Channel for Wave {
	fn step(&mut self, cycle: u16) {
		self.status.step(cycle);
	}
	
	fn next(&mut self, out_rate: f32) -> i8 {
		if !self.status.enabled || self.volume == 0 {
			0
		} else {
			self.sample_counter = (self.sample_counter + self.samples.samples_to_generate(out_rate)) % 32;
			let pattern = self.pattern[(self.sample_counter / 2) as usize];
			let pattern = if self.sample_counter % 2 == 1 { pattern & 0b1111 } else { pattern >> 4 };
			pattern.cast_signed() >> (self.volume - 1)
		}
	}
}

#[derive(Default)]
pub struct Noise {
	volume: ChannelVolume,
	status: ChannelStatus<0b11_1111>,
	lfsr: u16,
	frequency: u8,
	samples: Samples,
}

impl Noise {
	pub fn write_volume_register(&mut self, volume: u8) {
		self.volume.register = volume;
		self.status.set_dac_enabled((volume >> 3) != 0);
	}
	
	pub fn write_frequency_register(&mut self, value: u8) {
		self.frequency = value;
		let divider = value & 0b111;
		let divider = if divider == 0 { 0.5 } else { f32::from(divider) };
		self.samples.sample_rate = 262_144_f32 / divider / f32::from(1i16 << (value >> 4));
	}
	
	pub fn write_control_register(&mut self, control: u8) {
		if !self.status.dac_enabled { return; }
		if self.status.trigger(control) {
			self.volume.set_initial();
			self.lfsr = 0;
		}
	}
	
	fn read_register(&self, offset: u16) -> u8 {
		match offset {
			0 => 0xff,
			1 => self.volume.register,
			2 => self.frequency,
			3 => self.status.read_control_register(),
			_ => unreachable!()
		}
	}
	
	fn write_register(&mut self, offset: u16, value: u8) {
		match offset {
			0 => self.status.set_timer(value & 0b11_1111),
			1 => self.write_volume_register(value),
			2 => self.write_frequency_register(value),
			3 => self.write_control_register(value),
			_ => unreachable!()
		}
	}
}

impl Channel for Noise {
	fn step(&mut self, cycle: u16) {
		self.status.step(cycle);
		self.volume.envelope(cycle);
	}
	
	fn next(&mut self, out_rate: f32) -> i8 {
		if !self.status.enabled {
			return 0;
		}
		
		for _ in 0..self.samples.samples_to_generate(out_rate) {
			let new_value = !(((self.lfsr >> 1) & 1) ^ (self.lfsr & 1));
			self.lfsr = (self.lfsr | (new_value << 15)) >> 1;
			if (self.frequency >> 3) & 1 != 0 {
				self.lfsr = self.lfsr & !(1 << 7) | new_value << 7;
			}
		}
		
		if self.lfsr & 1 != 0 {
			self.volume.value
		} else {
			0
		}
	}
}

pub struct Audio {
	cycle: u16,
	square1: SquareWave,
	square2: SquareWave,
	wave: Wave,
	noise: Noise,
	pub enabled: bool,
	pub master_volume: u8,
	pub sound_panning: u8,
	sample_rate: f32,
	queue: AudioQueue<i8>,
	samples: Samples,
}

impl Audio {
	pub fn new(audio_subsystem: &sdl2::AudioSubsystem) -> Self {
		let queue = audio_subsystem.open_queue(None, &sdl2::audio::AudioSpecDesired {
			freq: Some(48000),
			channels: Some(2),
			samples: None,
		}).unwrap();
		queue.resume();
		#[allow(clippy::cast_precision_loss)]
		let sample_rate = queue.spec().freq as f32;
		
		let square1 = SquareWave::default();
		let square2 = SquareWave::default();
		let noise = Noise::default();
		let wave = Wave::default();
		
		let samples = Samples { sample_rate, ..Default::default() };
		Self { cycle: 0, enabled: false, queue, square1, square2, wave, noise, sample_rate, samples, master_volume: 0x77, sound_panning: 0xf3 }
	}
	
	fn channels(&mut self) -> [&mut dyn Channel; 4] {
		[&mut self.square1, &mut self.square2, &mut self.wave, &mut self.noise]
	}
	
	fn pan_channel(&self, offset: u8, values: [i8; 4]) -> i8 {
		let mut sample = 0i16;
		for (i, value) in values.iter().enumerate() {
			sample += i16::from(value * (self.sound_panning >> ((i as u8) + offset) & 1).cast_signed());
		}
		let volume = i16::from((self.master_volume >> offset) & 0b111);
		(sample * volume / 8).truncate()
	}
	
	pub fn step(&mut self) {
		let cycle = self.cycle.wrapping_add(1);
		for channel in self.channels() {
			channel.step(cycle);
		}
		self.cycle = cycle;
		
		#[allow(clippy::cast_precision_loss)]
		let audio: Vec<i8> = (0..self.samples.samples_to_generate(crate::CYCLES_PER_SECOND as f32)).flat_map(|_| {
			let sample_rate = self.sample_rate;
			let samples = self.channels().map(|channel| channel.next(sample_rate));
			[self.pan_channel(4, samples), self.pan_channel(0, samples)]
		}).collect();
		if !audio.is_empty() {
			self.queue.queue_audio(&audio).unwrap();
		}
	}
	
	pub fn read_register(&self, address: u16) -> u8 {
		match address {
			0xff10..=0xff14 => self.square1.read_register(address - 0xff10),
			0xff16..=0xff19 => self.square2.read_register(address - 0xff15),
			0xff1a..=0xff1e => self.wave.read_register(address - 0xff1a),
			0xff20..=0xff23 => self.noise.read_register(address - 0xff20),
			0xff30..=0xff40 => self.wave.pattern[usize::from(address - 0xff30)],
			mmio::AUDIO_VOLUME => self.master_volume,
			mmio::AUDIO_PANNING => self.sound_panning,
			mmio::AUDIO_CONTROL => {
				(u8::from(self.enabled) << 7) | (0b111 << 4) | (u8::from(self.square1.status.enabled)) | (u8::from(self.square2.status.enabled) << 1) | (u8::from(self.wave.status.enabled) << 2) | (u8::from(self.noise.status.enabled) << 3)
			}
			_ => {
				eprintln!("Audio::read_register called with invalid address {address:#x}");
				0xff
			}
		}
	}
	
	pub fn write_register(&mut self, address: u16, value: u8) {
		if !self.enabled && address != mmio::AUDIO_CONTROL { return; }
		match address {
			0xff10..=0xff14 => self.square1.write_register(address - 0xff10, value),
			0xff16..=0xff19 => self.square2.write_register(address - 0xff15, value),
			0xff1a..=0xff1e => self.wave.write_register(address - 0xff1a, value),
			0xff20..=0xff23 => self.noise.write_register(address - 0xff20, value),
			0xff30..0xff40 => self.wave.pattern[usize::from(address - 0xff30)] = value,
			mmio::AUDIO_VOLUME => self.master_volume = value,
			mmio::AUDIO_PANNING => self.sound_panning = value,
			mmio::AUDIO_CONTROL => {
				self.enabled = (value & (1 << 7)) != 0;
				if !self.enabled {
					self.master_volume = 0;
					self.sound_panning = 0;
					self.square1 = SquareWave::default();
					self.square2 = SquareWave::default();
					self.noise = Noise::default();
					self.wave = Wave::default();
				}
			}
			_ => eprintln!("Audio::write_register called with invalid address {address:#x}")
		}
	}
}