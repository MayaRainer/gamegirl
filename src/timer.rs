#[derive(Default)]
pub struct Timer {
	pub cycle: u16,
	pub counter: u8,
	pub modulo: u8,
	pub control: u8,
}

#[derive(PartialEq)]
pub enum TimerReturn {
	Overflow,
	NoOverflow,
}

impl Timer {
	#[must_use]
	pub fn divider(&self) -> u8 {
		(self.cycle / 64) as u8
	}
	
	pub fn step(&mut self) -> TimerReturn {
		self.cycle += 1;
		if self.cycle == 1 << 14 {
			self.cycle = 0;
		}

		if (self.control & (1 << 2)) != 0 {
			let clock = match self.control & 0b11 {
				0 => 256,
				1 => 4,
				2 => 16,
				3 => 64,
				_ => unreachable!()
			};
			if self.cycle.is_multiple_of(clock) {
				let (counter, overflow) = self.counter.overflowing_add(1);
				if overflow {
					self.counter = self.modulo;
					return TimerReturn::Overflow;
				}
				self.counter = counter;
			}
		}
		TimerReturn::NoOverflow
	}
}