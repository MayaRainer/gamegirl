use crate::{mmio, System};

mod flags {
	pub const CARRY: u8 = 1 << 4;
	pub const HALF_CARRY: u8 = 1 << 5;
	pub const SUBTRACTION: u8 = 1 << 6;
	pub const ZERO: u8 = 1 << 7;
	
	pub fn by_index(flags: u8, index: u8) -> bool {
		match index {
			0 => flags & ZERO == 0,
			1 => flags & ZERO != 0,
			2 => flags & CARRY == 0,
			3 => flags & CARRY != 0,
			_ => panic!("Invalid flag index"),
		}
	}
}

#[derive(Debug)]
pub enum InterruptsEnabled {
	Disabled,
	Enabling,
	Enabled
}

#[derive(Debug)]
pub struct CPU {
	bc: u16,
	de: u16,
	hl: u16,
	a: u8,
	flags: u8,
	sp: u16,
	pc: u16,
	interrupts_enabled: InterruptsEnabled,
	halted: bool,
	pub cycle: u16,
}

impl Default for CPU {
    fn default() -> Self {
			Self {
				pc: 0x100,
				a: 1,
				flags: 0,
				bc: 0x0013,
				de: 0x00d8,
				hl: 0x014d,
				sp: 0xfffe,
				interrupts_enabled: InterruptsEnabled::Disabled,
				halted: false,
				cycle: 0,
			}
    }
}

impl CPU {
	fn read(&mut self, sys: &mut System, address: u16) -> u8 {
		self.cycle();
		sys.read(address)
	}
	
	fn read_u16(&mut self, sys: &mut System, address: u16) -> u16 {
		u16::from_le_bytes([self.read(sys, address), self.read(sys, address + 1)])
	}
	
	fn next(&mut self, sys: &mut System) -> u8 {
		let value = self.read(sys, self.pc);
		self.pc = self.pc.wrapping_add(1);
		value
	}
	
	fn next_u16(&mut self, sys: &mut System) -> u16 {
		let value = self.read_u16(sys, self.pc);
		self.pc = self.pc.wrapping_add(2);
		value
	}
	
	fn jump(&mut self, address: u16) {
		self.pc = address;
		self.cycle();
	}
	
	fn read_relative_jump_address(&mut self, sys: &mut System) -> u16 {
		let offset = i16::from(self.next(sys).cast_signed());
		self.pc.wrapping_add_signed(offset)
	}
	
	fn by_index(&self, index: u8) -> u16 {
		match index {
			0 => self.bc,
			1 => self.de,
			2 => self.hl,
			3 => self.sp,
			_ => panic!("Invalid index {index}"),
		}
	}
	
	fn by_index_mut(&mut self, index: u8) -> &mut u16 {
		match index {
			0 => &mut self.bc,
			1 => &mut self.de,
			2 => &mut self.hl,
			3 => &mut self.sp,
			_ => panic!("Invalid index {index}"),
		}
	}
	
	fn by_index_8bit(&mut self, sys: &mut System, index: u8) -> u8 {
		match index {
			0 | 2 | 4 => (self.by_index(index / 2) >> 8) as u8,
			1 | 3 | 5 => self.by_index(index / 2) as u8,
			6 => self.read(sys, self.hl),
			7 => self.a,
			_ => panic!("Invalid index {index}"),
		}
	}
	
	fn write(&mut self, sys: &mut System, address: u16, value: u8) {
		self.cycle();
		sys.write(address, value);
	}
	
	fn write_by_index_8bit(&mut self, sys: &mut System, index: u8, value: u8) {
		match index {
			0..6 => {
				let register = self.by_index_mut(index / 2);
				*register = if index.is_multiple_of(2) {
					u16::from(value) << 8 | *register & 0xff
				} else {
					*register & 0xff00 | u16::from(value)
				}
			}
			6 => self.write(sys, self.hl, value),
			7 => self.a = value,
			_ => panic!("Invalid index {index}"),
		}
	}
	
	fn push(&mut self, sys: &mut System, value: u16) {
		let [low, high] = value.to_le_bytes();
		self.sp = self.sp.wrapping_sub(1);
		self.write(sys, self.sp, high);
		self.sp = self.sp.wrapping_sub(1);
		self.write(sys, self.sp, low);
	}
	
	fn pop(&mut self, sys: &mut System) -> u16 {
		let value = self.read_u16(sys, self.sp);
		self.sp += 2;
		value
	}
	
	fn add(&mut self, register: u8, value: u8, use_carry: bool) -> u8 {
		let carry = use_carry && self.flags & flags::CARRY != 0;
		let half_carry = (register & 0x0f) + (value & 0x0f) + u8::from(carry) > 0x0f;
		let (result, new_carry) = register.carrying_add(value, carry);
		self.flags = (flags::ZERO * u8::from(result == 0))
			| (flags::CARRY * u8::from(new_carry))
			| (flags::HALF_CARRY * u8::from(half_carry));
		result
	}
	
	fn subtract(&mut self, register: u8, value: u8, use_carry: bool) -> u8 {
		let borrow = use_carry && self.flags & flags::CARRY != 0;
		let (_, half_carry) = (register << 4).borrowing_sub(value << 4, borrow);
		let (result, carry) = register.borrowing_sub(value, borrow);
		self.flags = flags::SUBTRACTION
			| (flags::ZERO * u8::from(result == 0))
			| (flags::CARRY * u8::from(carry))
			| (flags::HALF_CARRY * u8::from(half_carry));
		result
	}
	
	fn and(&mut self, register: u8, value: u8) -> u8 {
		let result = register & value;
		self.flags = flags::HALF_CARRY | (flags::ZERO * u8::from(result == 0));
		result
	}
	
	fn or(&mut self, register: u8, value: u8) -> u8 {
		let result = register | value;
		self.flags = flags::ZERO * u8::from(result == 0);
		result
	}
	
	fn xor(&mut self, register: u8, value: u8) -> u8 {
		let result = register ^ value;
		self.flags = flags::ZERO * u8::from(result == 0);
		result
	}
	
	fn rotate_left(&mut self, value: u8) -> u8 {
		let result = value.rotate_left(1);
		self.flags = (result & 1) * flags::CARRY;
		result
	}
	
	fn rotate_left_through_carry(&mut self, value: u8) -> u8 {
		let carry = u8::from(self.flags & flags::CARRY != 0);
		(self.rotate_left(value) & !0b1) | carry
	}
	
	fn rotate_right(&mut self, value: u8) -> u8 {
		self.flags = (value & 1) * flags::CARRY;
		value.rotate_right(1)
	}
	
	fn rotate_right_through_carry(&mut self, value: u8) -> u8 {
		let carry = u8::from(self.flags & flags::CARRY != 0);
		(self.rotate_right(value) & !(1 << 7)) | (carry << 7)
	}
	
	fn shift_right(&mut self, value: u8) -> u8 {
		let result = value >> 1;
		self.flags = ((value & 0x1) * flags::CARRY) | (u8::from(result == 0) * flags::ZERO);
		result
	}
	
	fn add_signed_8bit_to_sp(&mut self, value: u8) -> u16 {
		let carry_value = u16::from(value);
		let half_carry = (self.sp & 0x0f) + (carry_value & 0x0f) > 0x0f;
		let carry = (self.sp & 0xff) + carry_value > 0xff;
		self.flags = (flags::CARRY * u8::from(carry)) | (flags::HALF_CARRY * u8::from(half_carry));
		self.cycle();
		self.sp.wrapping_add_signed(i16::from(value.cast_signed()))
	}
	
	fn cycle(&mut self) {
		self.cycle = self.cycle.wrapping_add(1);
	}
	
	pub(crate) fn step(&mut self, sys: &mut System) {
		let interrupts = sys.read(mmio::PENDING_INTERRUPTS);
		if let Some(interrupt) = (interrupts & 0b11111 & sys.read(mmio::ENABLED_INTERRUPTS)).lowest_one() {
			self.halted = false;
			if matches!(self.interrupts_enabled, InterruptsEnabled::Enabled) {
				self.cycle();
				self.cycle();
				self.push(sys, self.pc);
				sys.write(mmio::PENDING_INTERRUPTS, interrupts & !(1 << interrupt));
				self.jump(0x40 + (interrupt as u16) * 8);
				self.interrupts_enabled = InterruptsEnabled::Disabled;
			}
		}
		
		if self.halted {
			self.cycle();
			return;
		}
		if matches!(self.interrupts_enabled, InterruptsEnabled::Enabling) {
			self.interrupts_enabled = InterruptsEnabled::Enabled;
		}
		
		let opcode = self.next(sys);
		match opcode {
			0x00 /* NOP */ => {},
			0x01 | 0x11 | 0x21 | 0x31 /* LD r16, n16 */ => {
				*self.by_index_mut((opcode - 1) / 0x10) = self.next_u16(sys);
			}
			0x02 | 0x12 => /* LD [BC]/[DE], A */ {
				self.write(sys, self.by_index((opcode - 2) / 0x10), self.a);
			}
			0x03 | 0x13 | 0x23 | 0x33 /* INC r16 */ => {
				self.cycle();
				let register = self.by_index_mut((opcode - 3) / 0x10);
				*register = register.wrapping_add(1);
			}
			0x04 | 0x0c | 0x14 | 0x1c | 0x24 | 0x2c | 0x34 | 0x3c /* INC r8/[HL] */ => {
				let index = (opcode - 0x04) / 0x08;
				let value = self.by_index_8bit(sys, index);
				let result = value.wrapping_add(1);
				self.flags = self.flags & flags::CARRY | (u8::from(result == 0) * flags::ZERO) | (u8::from(value & 0x0f == 0x0f) * flags::HALF_CARRY);
				self.write_by_index_8bit(sys, index, result);
			}
			0x05 | 0x0d | 0x15 | 0x1d | 0x25 | 0x2d | 0x35 | 0x3d /* DEC r8/[HL] */ => {
				let index = (opcode - 0x04) / 0x08;
				let value = self.by_index_8bit(sys, index);
				let result = value.wrapping_sub(1);
				self.flags = self.flags & flags::CARRY | flags::SUBTRACTION | (u8::from(result == 0) * flags::ZERO) | (u8::from(value.trailing_zeros() >= 4) * flags::HALF_CARRY);
				self.write_by_index_8bit(sys, index, result);
			}
			0x06 | 0x0e | 0x16 | 0x1e | 0x26 | 0x2e | 0x36 | 0x3e /* LD r8/[HL], r8 */ => {
				let value = self.next(sys);
				self.write_by_index_8bit(sys, (opcode - 0x06) / 0x08, value);
			}
			0x07 /* RLCA */ => {
				self.a = self.rotate_left(self.a);
			}
			0x08 /* LD [a16], SP */ => {
				let address = self.next_u16(sys);
				let [low, high] = self.sp.to_le_bytes();
				self.write(sys, address, low);
				self.write(sys, address + 1, high);
			}
			0x09 | 0x19 | 0x29 | 0x39 /* ADD HL, r16 */ => {
				self.cycle();
				let value = self.by_index((opcode - 9) / 0x10);
				let half_carry = (self.hl & 0xfff) + (value & 0xfff) > 0xfff;
				let (result, carry) = self.hl.overflowing_add(value);
				self.flags = self.flags & flags::ZERO | (flags::CARRY * u8::from(carry)) | (flags::HALF_CARRY * u8::from(half_carry));
				self.hl = result;
			}
			0x0a | 0x1a => /* LD A, [BC]/[DE] */ {
				self.a = self.read(sys, self.by_index((opcode - 2) / 0x10));
			}
			0x0b | 0x1b | 0x2b | 0x3b /* DEC r16 */ => {
				self.cycle();
				let register = self.by_index_mut((opcode - 0x0b) / 0x10);
				*register = register.wrapping_sub(1);
			}
			0x0f /* RRCA */ => {
				self.a = self.rotate_right(self.a);
			}
			0x10 /* STOP */ => eprintln!("STOP not implemented"),
			0x17 /* RLA */ => {
				self.a = self.rotate_left_through_carry(self.a);
			}
			0x18 /* JR e8 */ => {
				let target = self.read_relative_jump_address(sys);
				self.jump(target);
			}
			0x1f /* RRA */ => {
				self.a = self.rotate_right_through_carry(self.a);
			}
			0x20 | 0x28 | 0x30 | 0x38 /* JR cc, e8 */ => {
				let target = self.read_relative_jump_address(sys);
				if flags::by_index(self.flags, (opcode - 0x20) / 0x08) {
					self.jump(target);
				}
			}
			0x22 => /* LD [HL+], A */ {
				self.write(sys, self.hl, self.a);
				self.hl += 1;
			}
			0x27 /* DAA */ => {
				if self.flags & flags::SUBTRACTION != 0 {
					self.a = self.a.wrapping_sub(u8::from(self.flags & flags::CARRY != 0) * 0x60 + u8::from(self.flags & flags::HALF_CARRY != 0) * 0x6);
					self.flags = (self.flags & (flags::CARRY | flags::SUBTRACTION)) | (u8::from(self.a == 0) * flags::ZERO);
				} else {
					let carry = self.flags & flags::CARRY != 0 || self.a > 0x99;
					let half_carry = self.flags & flags::HALF_CARRY != 0 || self.a & 0x0f > 0x09;
					self.a = self.a.wrapping_add(u8::from(carry) * 0x60 + u8::from(half_carry) * 0x6);
					self.flags = (self.flags & (flags::SUBTRACTION)) | (u8::from(carry) * flags::CARRY) | (u8::from(self.a == 0) * flags::ZERO);
				}
			}
			0x2a => /* LD A, [HL+] */ {
				self.a = self.read(sys, self.hl);
				self.hl += 1;
			}
			0x2f /* CPL */ => {
				self.a = !self.a;
				self.flags |= flags::HALF_CARRY | flags::SUBTRACTION;
			}
			0x32 => /* LD [HL-], A */ {
				self.write(sys, self.hl, self.a);
				self.hl -= 1;
			}
			0x37 /* SCF */ => {
				self.flags &= !(flags::SUBTRACTION | flags::HALF_CARRY);
				self.flags |= flags::CARRY;
			}
			0x3a => /* LD A, [HL+] */ {
				self.a = self.read(sys, self.hl);
				self.hl -= 1;
			}
			0x3f /* CCF */ => {
				self.flags &= !(flags::SUBTRACTION | flags::HALF_CARRY);
				self.flags ^= flags::CARRY;
			}
			0x40..0x76 | 0x77..0x80 /* LD r8/[HL], r8/[HL] */ => {
				let value = self.by_index_8bit(sys, opcode % 8);
				self.write_by_index_8bit(sys, (opcode - 0x40) / 8, value);
			}
			0x76 /* HALT */ => {
				self.halted = true;
			}
			0x80..0x88 /* ADD A, r8/[HL] */ => {
				let value = self.by_index_8bit(sys, opcode - 0x80);
				self.a = self.add(self.a, value, false);
			}
			0x88..0x90 /* ADC A, r8/[HL] */ => {
				let value = self.by_index_8bit(sys, opcode - 0x88);
				self.a = self.add(self.a, value, true);
			}
			0x90..0x98 /* SUB A, r8/[HL] */ => {
				let value = self.by_index_8bit(sys, opcode - 0x90);
				self.a = self.subtract(self.a, value, false);
			}
			0x98..0xa0 /* SBC A, r8/[HL] */ => {
				let value = self.by_index_8bit(sys, opcode - 0x98);
				self.a = self.subtract(self.a, value, true);
			}
			0xa0..0xa8 /* AND A, r8/[HL] */ => {
				let value = self.by_index_8bit(sys, opcode - 0xa0);
				self.a = self.and(self.a, value);
			}
			0xa8..0xb0 /* XOR A, r8/[HL] */ => {
				let value = self.by_index_8bit(sys, opcode - 0xa8);
				self.a = self.xor(self.a, value);
			}
			0xb0..0xb8 /* OR A, r8/[HL] */ => {
				let value = self.by_index_8bit(sys, opcode - 0xb0);
				self.a = self.or(self.a, value);
			}
			0xb8..0xc0 /* CP A, r8/[HL] */ => {
				let value = self.by_index_8bit(sys, opcode - 0xb8);
				self.subtract(self.a, value, false);
			}
			0xc0 | 0xc8 | 0xd0 | 0xd8 /* RET cc */ => {
				if flags::by_index(self.flags, (opcode - 0xc0) / 0x08) {
					let target = self.pop(sys);
					self.jump(target);
				}
				self.cycle();
			}
			0xc1 | 0xd1 | 0xe1 /* POP r16 */ => {
				*self.by_index_mut((opcode - 0xc1) / 0x10) = self.pop(sys);
			}
			0xc2 | 0xca | 0xd2 | 0xda /* JP cc, a16 */ => {
				let target = self.next_u16(sys);
				if flags::by_index(self.flags, (opcode - 0xc2) / 0x08) {
					self.jump(target);
				}
			}
			0xc3 /* JP a16 */ => {
				let target = self.next_u16(sys);
				self.jump(target);
			}
			0xc4 | 0xcc | 0xd4 | 0xdc /* CALL cc, a16 */ => {
				let target = self.next_u16(sys);
				if flags::by_index(self.flags, (opcode - 0xc4) / 0x08) {
					self.push(sys, self.pc);
					self.jump(target);
				}
			}
			0xc5 | 0xd5 | 0xe5 /* PUSH r16 */ => {
				self.push(sys, self.by_index((opcode - 0xc5) / 0x10));
				self.cycle();
			}
			0xc6 /* ADD A, n8 */ => {
				let value = self.next(sys);
				self.a = self.add(self.a, value, false);
			}
			0xc7 | 0xcf | 0xd7 | 0xdf | 0xe7 | 0xef | 0xf7 | 0xff /* RST vec */ => {
				self.push(sys, self.pc);
				self.jump(u16::from(opcode - 0xc7));
			}
			0xc9 /* RET */ => {
				let target = self.pop(sys);
				self.jump(target);
			}
			0xcb /* PREFIX */ => {
				let opcode = self.next(sys);
				match opcode {
					0x00..0x08 /* RLC r8/[HL] */ => {
						let operand = self.by_index_8bit(sys, opcode);
						let value = self.rotate_left(operand);
						self.flags |= flags::ZERO * u8::from(value == 0);
						self.write_by_index_8bit(sys, opcode, value);
					}
					0x08..0x10 /* RRC r8/[HL] */ => {
						let index = opcode - 0x08;
						let operand = self.by_index_8bit(sys, index);
						let value = self.rotate_right(operand);
						self.flags |= flags::ZERO * u8::from(value == 0);
						self.write_by_index_8bit(sys, index, value);
					}
					0x10..0x18 /* RL r8/[HL] */ => {
						let index = opcode - 0x10;
						let operand = self.by_index_8bit(sys, index);
						let value = self.rotate_left_through_carry(operand);
						self.flags |= flags::ZERO * u8::from(value == 0);
						self.write_by_index_8bit(sys, index, value);
					}
					0x18..0x20 /* RR r8/[HL] */ => {
						let index = opcode - 0x18;
						let operand = self.by_index_8bit(sys, index);
						let value = self.rotate_right_through_carry(operand);
						self.flags |= flags::ZERO * u8::from(value == 0);
						self.write_by_index_8bit(sys, index, value);
					}
					0x20..0x28 /* SLA r8/[HL] */ => {
						let index = opcode - 0x20;
						let mut value = self.by_index_8bit(sys, index);
						let carry = value & 0x80;
						value <<= 1;
						self.write_by_index_8bit(sys, index, value);
						self.flags = (u8::from(carry != 0) * flags::CARRY) | (u8::from(value == 0) * flags::ZERO);
					}
					0x28..0x30 /* SRA r8/[HL] */ => {
						let index = opcode - 0x28;
						let value = self.by_index_8bit(sys, index);
						let high = value & 0x80;
						let result = self.shift_right(value) | high;
						self.write_by_index_8bit(sys, index, result);
					}
					0x30..0x38 /* SWAP r8/[HL] */ => {
						let index = opcode - 0x30;
						let value = self.by_index_8bit(sys, index);
						self.flags = u8::from(value == 0) * flags::ZERO;
						let high = value & 0xf0;
						self.write_by_index_8bit(sys, index, value << 4 | high >> 4);
					}
					0x38..0x40 /* SRL r8/[HL] */ => {
						let index = opcode - 0x38;
						let operand = self.by_index_8bit(sys, index);
						let value = self.shift_right(operand);
						self.write_by_index_8bit(sys, index, value);
					}
					0x40..=0x7f /* BIT u3, r8/[HL] */ => {
						let zero = u8::from(self.by_index_8bit(sys, opcode % 8) & (1 << ((opcode - 0x40) / 8)) == 0);
						self.flags = (self.flags & flags::CARRY) | flags::HALF_CARRY | (flags::ZERO * zero);
					}
					0x80..=0xbf /* RES u3, r8/[HL] */ => {
						let index = opcode % 8;
						let value = self.by_index_8bit(sys, index);
						self.write_by_index_8bit(sys, index, value & !(1 << ((opcode - 0x80) / 8)));
					}
					0xc0..=0xff /* SET u3, r8/[HL] */ => {
						let index = opcode % 8;
						let value = self.by_index_8bit(sys, index);
						self.write_by_index_8bit(sys, index, value | 1 << ((opcode - 0xc0) / 8));
					}
				}
			}
			0xcd /* CALL a16 */ => {
				let address = self.next_u16(sys);
				self.push(sys, self.pc);
				self.jump(address);
			}
			0xce /* ADC A, n8 */ => {
				let value = self.next(sys);
				self.a = self.add(self.a, value, true);
			}
			0xd6 /* SUB A, n8 */ => {
				let value = self.next(sys);
				self.a = self.subtract(self.a, value, false);
			}
			0xd9 /* RETI */ => {
				self.interrupts_enabled = InterruptsEnabled::Enabled;
				let target = self.pop(sys);
				self.jump(target);
			}
			0xde /* SBC A, n8 */ => {
				let value = self.next(sys);
				self.a = self.subtract(self.a, value, true);
			}
			0xe0 /* LDH [a8], A */ => {
				let address = 0xff00 + u16::from(self.next(sys));
				self.write(sys, address, self.a);
			}
			0xe2 /* LDH [C], A */ => {
				let address = 0xff00 + (self.bc & 0xff);
				self.write(sys, address, self.a);
			}
			0xe6 /* AND A, n8 */ => {
				let value = self.next(sys);
				self.a = self.and(self.a, value);
			}
			0xe8 /* ADD SP, e8 */ => {
				let value = self.next(sys);
				self.sp = self.add_signed_8bit_to_sp(value);
				self.cycle();
			}
			0xe9 /* JP HL */ => {
				self.pc = self.hl;
			}
			0xea /* LD [a16], A */ => {
				let address = self.next_u16(sys);
				self.write(sys, address, self.a);
			}
			0xee /* XOR A, n8 */ => {
				let value = self.next(sys);
				self.a = self.xor(self.a, value);
			}
			0xf0 /* LDH A, [a8] */ => {
				let address = 0xff00 + u16::from(self.next(sys));
				self.a = self.read(sys, address);
			}
			0xf1 /* POP AF */ => {
				let [f, a] = self.pop(sys).to_le_bytes();
				self.flags = f & 0xf0;
				self.a = a;
			}
			0xf2 /* LDH A, [C] */ => {
				let address = 0xff00 + (self.bc & 0xff);
				self.a = self.read(sys, address);
			}
			0xf3 /* DI */ => {
				self.interrupts_enabled = InterruptsEnabled::Disabled;
			}
			0xf5 /* PUSH AF */ => {
				self.push(sys, u16::from_le_bytes([self.flags, self.a]));
				self.cycle();
			}
			0xf6 /* OR A, n8 */ => {
				let value = self.next(sys);
				self.a = self.or(self.a, value);
			}
			0xf8 /* LD HL, SP + e8 */ => {
				let value = self.next(sys);
				self.hl = self.add_signed_8bit_to_sp(value);
			}
			0xf9 /* LD SP, HL */ => {
				self.sp = self.hl;
				self.cycle();
			}
			0xfb /* EI */ => {
				if matches!(self.interrupts_enabled ,InterruptsEnabled::Disabled) {
					self.interrupts_enabled = InterruptsEnabled::Enabling;
				}
			}
			0xfa /* LD A, [a16] */ => {
				let address = self.next_u16(sys);
				self.a = self.read(sys, address);
			}
			0xfe /* CP A, n8 */ => {
				let operand = self.next(sys);
				self.subtract(self.a, operand, false);
			}
			_ => panic!("Invalid opcode {opcode}"),
		}
	}
}