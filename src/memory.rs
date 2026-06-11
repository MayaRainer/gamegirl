use std::io::Read;
use std::path::{PathBuf};

pub trait MemoryMapper {
	fn read(&self, address: u16) -> u8;
	fn write(&mut self, address: u16, value: u8);
	fn save(&self);
}

struct Shared {
	rom: Vec<u8>,
	ram: Vec<u8>,
	rom_bank: u8,
	ram_bank: u8,
	ram_enabled: bool,
	save_path: Option<PathBuf>,
}

#[derive(Clone, Copy)]
enum RamConfiguration {
	Internal,
	External,
	ExternalWithBattery,
}

impl Shared {
	#[must_use]
	pub fn new(rom: Vec<u8>, ram_configuration: RamConfiguration) -> Self {
		let ram_size = if matches!(ram_configuration, RamConfiguration::Internal) {
			8
		} else {
			match rom[0x149] {
				0 | 2 => 8,
				3 => 32,
				_ => unimplemented!()
			}
		};
		let mut ram = vec![0; 0x1000 * ram_size];
		let save_path = if matches!(ram_configuration, RamConfiguration::ExternalWithBattery) {
			let title = String::from_utf8(rom[0x134..=0x143].iter().copied().take_while(|c| *c > 0).collect()).expect("Invalid rom format");
			let save_path = ["saves", &format!("{title}.sav")].iter().collect();
			if let Ok(mut file) = std::fs::File::open(&save_path) {
				file.read_exact(&mut ram).expect("Invalid save file");
				println!("Loaded save file");
			}
			Some(save_path)
		} else { None };
		Self { rom, ram, rom_bank: 1, ram_bank: 0, ram_enabled: false, save_path }
	}
	
	fn read(&self, address: u16) -> u8 {
		match address {
			0x0000..0x4000 => self.rom[usize::from(address)],
			0x4000..0x8000 => {
				self.rom[usize::from(address).wrapping_add_signed(0x4000 * (isize::from(self.rom_bank) - 1))]
			}
			0xa000..0xc000 => {
				if self.ram_enabled {
					self.ram[usize::from(address - 0x8000) + 0x4000 * usize::from(self.ram_bank)]
				} else {
					0xff
				}
			}
			address => {
				self.ram[usize::from(address - 0x8000)]
			}
		}
	}
	
	fn write(&mut self, address: u16, value: u8) {
		match address {
			0x0000..0x8000 => {
				eprintln!("Attempt to write to ROM at address {address}");
			}
			0xa000..0xc000 => {
				if self.ram_enabled {
					self.ram[usize::from(address - 0x8000)] = value;
				}
			}
			address => self.ram[usize::from(address - 0x8000)] = value,
		}
	}
	
	fn save(&self) {
		if let Some(save_path) = &self.save_path {
			std::fs::write(save_path, &self.ram).expect("Error writing save file");
		}
	}
}

pub struct RomOnly(Shared);

impl RomOnly {
	#[must_use]
	pub fn new(rom: Vec<u8>) -> Self {
		let mut shared = Shared::new(rom, RamConfiguration::Internal);
		shared.ram_enabled = true;
		Self(shared)
	}
}

impl MemoryMapper for RomOnly {
	fn read(&self, address: u16) -> u8 {
		self.0.read(address)
	}
	
	fn write(&mut self, address: u16, value: u8) {
		self.0.write(address, value);
	}
	
	fn save(&self) {}
}

pub struct MBC1(Shared);

impl MBC1 {
	#[must_use]
	pub fn new(rom: Vec<u8>) -> Self {
		let external_ram = match rom[0x147] {
			1 => RamConfiguration::Internal,
			2 => RamConfiguration::External,
			3 => RamConfiguration::ExternalWithBattery,
			_ => unreachable!()
		};
		Self(Shared::new(rom, external_ram))
	}
}

impl MemoryMapper for MBC1 {
	fn read(&self, address: u16) -> u8 {
		self.0.read(address)
	}
	
	fn write(&mut self, address: u16, value: u8) {
		match address {
			0x0000..0x2000 => {
				self.0.ram_enabled = (value & 0x0f) == 0x0a;
			}
			0x2000..0x4000 => {
				let bank = value & 0b11111;
				self.0.rom_bank = if bank == 0 { 1 } else { bank % (self.0.rom.len() / 0x4000) as u8 };
			}
			0x4000..0x6000 => {
				if self.0.ram.len() > 0x8000 {
					self.0.ram_bank = value & 0b11;
				}
			}
			0x6000..0x8000 => {
				eprintln!("Banking Mode select not implemented");
			}
			address => self.0.write(address, value),
		}
	}
	
	fn save(&self) {
		self.0.save();
	}
}

pub struct MBC3(Shared);

impl MBC3 {
	#[must_use]
	pub fn new(rom: Vec<u8>) -> Self {
		let external_ram = match rom[0x147] {
			0x11 => RamConfiguration::Internal,
			0x12 => RamConfiguration::External,
			0x13 => RamConfiguration::ExternalWithBattery,
			_ => unreachable!()
		};
		Self(Shared::new(rom, external_ram))
	}
}

impl MemoryMapper for MBC3 {
	fn read(&self, address: u16) -> u8 {
		self.0.read(address)
	}
	
	fn write(&mut self, address: u16, value: u8) {
		match address {
			0x0000..0x2000 => {
				self.0.ram_enabled = (value & 0x0f) == 0x0a;
			}
			0x2000..0x4000 => {
				self.0.rom_bank = if value == 0 { 1 } else { value };
			}
			0x4000..0x6000 => {
				if value < 0x08 {
					if self.0.ram.len() > 0x8000 {
						self.0.ram_bank = value;
					}
				} else {
					eprintln!("RTC not implemented");
				}
			}
			0x6000..0x8000 => {
				eprintln!("RTC not implemented");
			}
			address => self.0.write(address, value),
		}
	}
	
	fn save(&self) {
		self.0.save();
	}
}