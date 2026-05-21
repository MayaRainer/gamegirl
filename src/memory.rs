pub trait MemoryMapper {
	fn read(&self, address: u16) -> u8;
	fn write(&mut self, address: u16, value: u8);
}

pub struct RomOnly {
	pub rom: Vec<u8>,
	pub ram: Vec<u8>,
}

impl MemoryMapper for RomOnly {
	fn read(&self, address: u16) -> u8 {
		match address {
			0x0000..0x8000 => self.rom[usize::from(address)],
			address => self.ram[usize::from(address - 0x8000)],
		}
	}
	
	fn write(&mut self, address: u16, value: u8) {
		match address {
			0x0000..0x8000 => {
				eprintln!("Attempt to write to ROM at address {address}");
			}
			address => self.ram[usize::from(address - 0x8000)] = value,
		}
	}
}

pub struct MBC1 {
	rom: Vec<u8>,
	ram: Vec<u8>,
	rom_bank: u8,
	ram_bank: u8,
	ram_enabled: bool,
}

impl MBC1 {
	#[must_use]
	pub fn new(rom: Vec<u8>) -> Self {
		let ram_size = if rom[0x147] == 2 {
			match rom[0x149] {
				0 | 2 => 8,
				3 => 32,
				_ => unimplemented!()
			}
		} else { 8 };
		Self { rom, ram: vec![0; 0x1000 * ram_size], rom_bank: 1, ram_bank: 0, ram_enabled: false }
	}
}

impl MemoryMapper for MBC1 {
	fn read(&self, address: u16) -> u8 {
		match address {
			0x0000..0x4000 => self.rom[usize::from(address)],
			0x4000..0x8000 => {
				self.rom[usize::from(address).wrapping_add_signed(0x4000 * (isize::from(self.rom_bank) - 1))]
			},
			0xa000..0xc000 => {
				if self.ram_enabled {
					self.ram[usize::from(address - 0x8000) + 0x4000 * usize::from(self.ram_bank)]
				} else {
					0xff
				}
			}
			address => {
				self.ram[usize::from(address - 0x8000)]
			},
		}
	}
	
	fn write(&mut self, address: u16, value: u8) {
		match address {
			0x0000..0x2000 => {
				self.ram_enabled = (value & 0x0f) == 0x0a;
			}
			0x2000..0x4000 => {
				let bank = value & 0b11111;
				self.rom_bank = if bank == 0 { 1 } else { bank % (self.rom.len() / 0x4000) as u8 };
			}
			0x4000..0x6000 => {
				if self.ram.len() > 0x8000 {
					self.ram_bank = value & 0b11;
				}
			}
 			0x6000..0x8000 => {
				unimplemented!();
			},
			0xa000..0xc000 => {
				if self.ram_enabled {
					self.ram[usize::from(address - 0x8000)] = value;
				}
			}
			address => self.ram[usize::from(address - 0x8000)] = value,
		}
	}
}