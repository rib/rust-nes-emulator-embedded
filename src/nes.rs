use crate::constants::*;
use crate::prelude::*;
use crate::system::*;
use crate::cpu::*;
use crate::ppu::*;
use log::{warn};

pub struct Nes {
    pixel_format: PixelFormat,
    cpu: Cpu,
    cpu_clock: u64,
    ppu_clock: u64,
    apu: Apu,
    system: System,
    framebuffers: Vec<Vec<u8>>,
    current_fb: usize,
}

impl Nes {
    pub fn new(pixel_format: PixelFormat) -> Nes {
        let cpu = Cpu::default();
        let mut ppu = Ppu::default();
        let mut apu = Apu::default();
        ppu.draw_option.fb_width = FRAMEBUFFER_WIDTH as u32;
        ppu.draw_option.fb_height = FRAMEBUFFER_HEIGHT as u32;
        ppu.draw_option.offset_x = 0;
        ppu.draw_option.offset_y = 0;
        ppu.draw_option.scale = 1;
        ppu.draw_option.pixel_format = pixel_format;

        let mut framebuffers = vec![vec![0u8; FRAMEBUFFER_WIDTH * FRAMEBUFFER_HEIGHT * 4]; 2];

        let system = System::new(ppu, Cartridge::none());
        Nes {
            pixel_format, cpu, cpu_clock: 0, ppu_clock: 0, apu, system, framebuffers, current_fb: 0
        }
    }

    pub fn insert_cartridge(&mut self, cartridge: Option<Cartridge>) {
        if let Some(cartridge) = cartridge {
            self.system.cartridge = cartridge;
        } else {
            self.system.cartridge = Cartridge::none();
        }
    }

    /// Assumes everything is Default initialized beforehand
    pub fn poweron(&mut self) {
        self.cpu.interrupt(&mut self.system, Interrupt::RESET);
    }

    pub fn reset(&mut self) {
        self.cpu.p |= Flags::INTERRUPT;
        self.cpu.sp = self.cpu.sp.wrapping_sub(3);
        self.cpu.interrupt(&mut self.system, Interrupt::RESET);
    }

    pub fn system_mut(&mut self) -> &mut System {
        &mut self.system
    }
    pub fn system_cpu(&mut self) -> &mut Cpu {
        &mut self.cpu
    }
    pub fn system_ppu(&mut self) -> &mut Ppu {
        &mut self.system.ppu
    }
    pub fn debug_read_ppu(&mut self, addr: u16) -> u8 {
        self.system.ppu.read_u8(&mut self.system.cartridge, addr)
    }

    pub fn allocate_framebuffer(&self) -> Framebuffer {
        Framebuffer::new(FRAMEBUFFER_WIDTH, FRAMEBUFFER_HEIGHT, self.pixel_format)
    }

    // Aiming for Meson compatible trace format which can be used for cross referencing
    #[cfg(feature="trace")]
    fn display_trace(&self) {
        let trace = &self.cpu.trace;
        let pc = trace.instruction_pc;
        let op = trace.instruction_op_code;
        let operand_len = trace.instruction.len() - 1;
        let bytecode_str = if operand_len == 2 {
            let lsb = trace.instruction_operand & 0xff;
            let msb = (trace.instruction_operand & 0xff00) >> 8;
            format!("${op:02X} ${lsb:02X} ${msb:02X}")
        } else if operand_len == 1{
            format!("${op:02X} ${:02X}", trace.instruction_operand)
        } else {
            format!("${op:02X}")
        };
        let disassembly = trace.instruction.disassemble(trace.instruction_operand, trace.effective_address, trace.loaded_mem_value, trace.stored_mem_value);
        let a = trace.saved_a;
        let x = trace.saved_x;
        let y = trace.saved_y;
        let sp = trace.saved_sp & 0xff;
        let p = trace.saved_p.to_flags_string();
        let cpu_cycles = trace.saved_cyc;
        println!("{pc:0X} {bytecode_str:11} {disassembly:23} A:{a:02X} X:{x:02X} Y:{y:02X} P:{p} SP:{sp:X} CPU Cycle:{cpu_cycles}");

    }

    pub fn tick_frame(&mut self, mut framebuffer: Framebuffer) {
        let rental = framebuffer.rent_data();
        if let Some(mut fb_data) = rental {
            let fb = fb_data.data.as_mut_ptr();
            'frame_loop: loop {

                // We treat the CPU as our master clock and the PPU is driven according
                // to the forward progress of the CPU's clock.

                // For now just assuming NTSC which has an exact 1:3 ratio between cpu
                // clocks and PPU...
                let expected_ppu_clock = self.cpu_clock * 3;
                let ppu_delta = expected_ppu_clock - self.ppu_clock;

                // Let the PPU catch up with the CPU clock before progressing the CPU
                // in case we need to quit to allow a redraw (so we will resume
                // catching afterwards)
                for _ in 0..ppu_delta {
                    let status = self.system.step_ppu(self.ppu_clock, fb);
                    self.ppu_clock += 1;
                    match status {
                        PpuStatus::None => { continue },
                        PpuStatus::FinishedFrame => { break 'frame_loop; },
                        PpuStatus::RaiseNmi => {
                            //println!("VBLANK NMI");
                            self.cpu.interrupt(&mut self.system, Interrupt::NMI);
                        }
                    }
                }

                if self.system.oam_dma_cpu_suspend_cycles == 0 {
                    self.cpu_clock += self.cpu.step(&mut self.system) as u64;
                } else {
                    self.cpu_clock += self.system.oam_dma_cpu_suspend_cycles as u64;
                    self.system.oam_dma_cpu_suspend_cycles = 0;
                };

                #[cfg(feature="trace")]
                self.display_trace();
            }
        } else {
            warn!("Can't tick with framebuffer that's still in use!");
        }
    }
}