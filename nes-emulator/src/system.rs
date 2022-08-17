use crate::apu::apu::Apu;
use crate::ppu::Ppu;

#[cfg(feature="ppu-sim")]
use crate::ppusim::PpuSim;
use crate::trace::TraceEvent;

use super::constants::*;
use super::cartridge::*;
use super::port::*;
use bitflags::bitflags;

const WRAM_SIZE: usize = 0x0800;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Model {
    #[default]
    Ntsc,
    Pal
}
impl Model {
    pub fn cpu_clock_hz(&self) -> u32 {
        match self {
            Model::Ntsc => NTSC_CPU_CLOCK_HZ,
            Model::Pal => PAL_CPU_CLOCK_HZ,
        }
    }
}

bitflags! {
    pub struct WatchOps: u8 {
        const READ =  0b1;
        const WRITE = 0b10;
        const EXECUTE = 0b100;

        /// Also watches the superfluous, dummy reads/writes the CPU does
        const DUMMY = 0b1000;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchPoint {
    pub address: u16,
    pub ops: WatchOps,
}

/// The DMC doesn't have direct access to the system bus within
/// apu.step() so any DMA needs to requested, and the result
/// will be passed back
#[derive(Clone, Copy)]
pub struct DmcDmaRequest {
    pub address: u16
}

#[derive(Default, Clone, Copy)]
pub struct IoStatsRecord {
    reads: u64,
    writes: u64,
    execute: u64,
}

// State we don't want to capture in a snapshot/clone of the system
#[derive(Default)]
pub struct NoCloneDebugState {
    #[cfg(feature="debugger")]
    pub watch_points: Vec<WatchPoint>,
    #[cfg(feature="debugger")]
    pub watch_hit: bool,

    #[cfg(feature="io-stats")]
    pub io_stats: Vec<IoStatsRecord>
}
impl Clone for NoCloneDebugState {
    fn clone(&self) -> Self {
        Self::default()
    }
}

#[derive(Clone)]
pub struct System {
    pub ppu: Ppu,

    #[cfg(feature="ppu-sim")]
    pub ppu_sim: PpuSim,

    pub apu: Apu,

    /// Any time we step the APU we might get a DMA request from the DMC channel
    /// which the CPU needs to handle. The CPU will check this after any I/O
    /// or request to step the system.
    pub dmc_dma_request: Option<DmcDmaRequest>,

    pub open_bus_value: u8,

    /// 0x0000 - 0x07ff: WRAM
    /// 0x0800 - 0x1f7ff: WRAM  Mirror x3
    pub wram: [u8; WRAM_SIZE],

    pub cartridge: Cartridge,
    #[cfg(feature="ppu-sim")]
    pub ppu_sim_cartridge: Cartridge,

    pub port1: Port,
    pub port2: Port,

    pub debug: NoCloneDebugState,
}

impl System {

    pub fn new(model: Model, audio_sample_rate: u32, cartridge: Cartridge) -> Self {
        let ppu = Ppu::new(model);

        #[cfg(feature="ppu-sim")]
        let ppu_sim = PpuSim::new(model);

        let apu = Apu::new(model, audio_sample_rate);

        Self {
            ppu,

            #[cfg(feature="ppu-sim")]
            ppu_sim,

            apu,
            dmc_dma_request: None,

            #[cfg(feature="ppu-sim")]
            ppu_sim_cartridge: cartridge.clone(),
            cartridge,

            wram: [0; WRAM_SIZE],

            port1: Default::default(),
            port2: Default::default(),

            open_bus_value: 0,

            debug: NoCloneDebugState {
                watch_points: vec![],
                watch_hit: false,

                #[cfg(feature="io-stats")]
                io_stats: vec![IoStatsRecord::default(); u16::MAX as usize],
            }
        }
    }

    pub(crate) fn power_cycle(&mut self) {
        // Note we want to preserve any debugger state so we don't re-create
        // everything

        self.ppu.power_cycle();

        #[cfg(feature="ppu-sim")]
        self.ppu_sim.power_cycle();

        self.apu.power_cycle();

        #[cfg(feature="ppu-sim")]
        self.ppu_sim_cartridge.power_cycle();
        self.cartridge.power_cycle();

        self.port1.power_cycle();
        self.port2.power_cycle();

        let ppu = std::mem::take(&mut self.ppu);
        #[cfg(feature="ppu-sim")]
        let ppu_sim = std::mem::take(&mut self.ppu_sim);
        let apu = std::mem::take(&mut self.apu);
        let cartridge = std::mem::take(&mut self.cartridge);
        #[cfg(feature="ppu-sim")]
        let ppu_sim_cartridge = std::mem::take(&mut self.ppu_sim_cartridge);
        let pad1 = std::mem::take(&mut self.port1);
        let pad2 = std::mem::take(&mut self.port2);

        #[cfg(feature="debugger")]
        let watch_points = std::mem::take(&mut self.debug.watch_points);

        *self = Self {
            ppu,

            #[cfg(feature="ppu-sim")]
            ppu_sim,

            apu,
            dmc_dma_request: None,

            #[cfg(feature="ppu-sim")]
            ppu_sim_cartridge,
            cartridge,

            wram: [0; WRAM_SIZE],

            port1: pad1,
            port2: pad2,

            open_bus_value: 0,

            debug: NoCloneDebugState {
                #[cfg(feature="debugger")]
                watch_points,
                #[cfg(feature="debugger")]
                watch_hit: false,

                #[cfg(feature="io-stats")]
                io_stats: vec![IoStatsRecord::default(); u16::MAX as usize],
            }
        };
    }

    pub(crate) fn reset(&mut self) {
        self.ppu.reset();
        self.apu.reset();
        //self.pad1.reset();
        //self.pad2.reset();
        self.cartridge.reset();
    }

    pub fn nmi_line(&self) -> bool {
        self.ppu.nmi_interrupt_raised
    }

    pub fn irq_line(&self) -> bool {
        self.apu.irq() || self.cartridge.mapper.irq()
    }

    /// Apply the open bus bits and update the open bus value for future reads
    fn apply_open_bus_bits_mut(&mut self, mut value: u8, undefined_bits: u8) -> u8 {
        value = value & !undefined_bits;
        value |= self.open_bus_value & undefined_bits;
        self.open_bus_value = value;
        value
    }

    /// Apply the open bus bits without additional side effects (for peeking)
    fn apply_open_bus_bits(&self, mut value: u8, undefined_bits: u8) -> u8 {
        value = value & !undefined_bits;
        value |= self.open_bus_value & undefined_bits;
        value
    }

    #[inline(always)]
    fn check_watch_points(&mut self, addr: u16, ops: WatchOps) {
        #[cfg(feature="debugger")]
        if self.debug.watch_points.len() > 0 {
            for w in &self.debug.watch_points {
                if w.address == addr && w.ops.contains(ops) {
                    self.debug.watch_hit = true;
                    break;
                }
            }
        }
    }

    /// Perform a system bus read from the CPU
    ///
    /// Considering that all CPU IO takes one CPU clock cycle this will
    /// also step the APU and PPU attached to the system bus to help keep
    /// their clocks synchronized (we know this won't push their clock
    /// into the future)
    fn read(&mut self, addr: u16) -> u8 {
        let (value, undefined_bits) = match addr {
            0x0000..=0x1fff => { // RAM
                //println!("system read {addr:x}");
                // mirror support
                let index = usize::from(addr) % self.wram.len();
                (arr_read!(self.wram, index), 0)
            }
            0x2000..=0x3fff => { // PPU I/O
                // Send any reads to the simulator for their side effects
                #[cfg(feature="ppu-sim")]
                {
                    self.ppu_sim.system_bus_read_start(addr);
                    // TODO: we also need to step the ppu_sim forward so we can
                    // read back a value
                    // XXX: to make that practical we need to give the sim ownership
                    // over its framebuffer otherwise we'd have to thread the
                    // framebuffer through read()s
                    //self.ppu_sim.sim_progress_for_read();
                    //(self.ppu_sim.data_bus, 0)
                }

                // PPU read handles open bus behaviour, so we assume there
                // are no undefined bits at this point
                (self.ppu.system_bus_read(&mut self.cartridge, addr), 0)
            }
            0x4000..=0x401f => {  // APU I/O
                let index = usize::from(addr - 0x4000);
                match index {
                    0x14 => { // Write-only OAMDMA
                        (0, 0xff)
                    }
                    0x16 => (self.port1.read(), 0b1110_0000), // pad1
                    0x17 => (self.port2.read(), 0b1110_0000), // pad2
                    _ => {
                        self.apu.read(addr)
                    }
                }
            }
            _ => { // Cartridge
                //println!("calling cartridge read_u8 for {addr:x}");
                self.cartridge.system_bus_read(addr)
            }
        };

        //println!("Stepping system during CPU read");
        self.step_for_cpu_cycle();

        // If this is a PPU simulator read then we have to wait until after
        // stepping the PPU before we can access the value
        #[cfg(feature="ppu-sim")]
        {
            if let 0x2000..=0x3fff = addr {
                value = self.ppu_sim.data_bus;
            }
        }

        let value = self.apply_open_bus_bits_mut(value, undefined_bits);
        //if addr == 0x4016 {
        //    println!("Read $4016 as {value:02x} / {value:08b}");
        //}
        value
    }

    /// Perform a system bus read from the CPU, reading non-instruction data
    ///
    /// Considering that all CPU IO takes one CPU clock cycle this will
    /// also step the APU and PPU attached to the system bus to help keep
    /// their clocks synchronized (we know this won't push their clock
    /// into the future)
    pub fn cpu_read(&mut self, addr: u16) -> u8 {
        self.check_watch_points(addr, WatchOps::READ);

        #[cfg(feature="io-stats")]
        {
            self.debug.io_stats[addr as usize].reads += 1;
        }

        self.read(addr)
    }

    /// Handle various superfluous reads that the CPU does
    ///
    /// These reads can have side effects, such as modifying open bus data or
    /// affecting register state, so they are currently not optimized out but
    /// they are differentiated in case that will help with debugging features
    /// and we may be able to optimize out some of these reads later if we know
    /// they can't have side effects.
    pub fn dummy_cpu_read(&mut self, addr: u16) {
        self.check_watch_points(addr, WatchOps::DUMMY | WatchOps::READ);

        #[cfg(feature="io-stats")]
        {
            self.debug.io_stats[addr as usize].reads += 1;
        }

        self.read(addr);
    }

    /// Perform a system bus read from the CPU, to fetch part of an instruction
    pub fn cpu_fetch(&mut self, addr: u16) -> u8 {
        self.check_watch_points(addr, WatchOps::EXECUTE);

        #[cfg(feature="io-stats")]
        {
            self.debug.io_stats[addr as usize].execute += 1;
        }

        self.read(addr)
    }

    /// Read the system bus without side-effects
    ///
    /// Use this for debugging purposes to be able to inspect memory and registers
    /// without affecting any state.
    pub fn peek(&mut self, addr: u16) -> u8 {

        let (value, undefined_bits) = match addr {
            0x0000..=0x1fff => { // RAM
                let index = usize::from(addr) % self.wram.len();
                (arr_read!(self.wram, index), 0)
            }
            0x2000..=0x3fff => { // PPU I/O
                // PPU read handles open bus behaviour, so we assume there
                // are no undefined bits at this point
                (self.ppu.system_bus_peek(&mut self.cartridge, addr), 0)
            }
            0x4000..=0x401f => {  // APU I/O
                let index = usize::from(addr - 0x4000);
                match index {
                    0x14 => { // Write-only OAMDMA
                        (0, 0xff)
                    }
                    0x16 => (self.port1.peek(), 0b1110_0000), // pad1
                    0x17 => (self.port2.peek(), 0b1110_0000), // pad2
                    _ => {
                        self.apu.peek(addr)
                    }
                }
            }
            _ => { // Cartridge
                self.cartridge.system_bus_peek(addr)
            }
        };

        let value = self.apply_open_bus_bits(value, undefined_bits);
        value
    }

    /// Perform a system bus write from the CPU
    fn write(&mut self, addr: u16, data: u8) {
        match addr {
            0x0000..=0x1fff => { // RAM
                // mirror
                let index = usize::from(addr) % self.wram.len();
                arr_write!(self.wram, index, data);
            }
            0x2000..=0x3fff => { // PPU
                self.ppu.system_bus_write(&mut self.cartridge, addr, data);
                #[cfg(feature="ppu-sim")]
                self.ppu_sim.system_bus_write_start(addr, data);
            }
            0x4000..=0x401f => {  // APU + I/O
                let index = usize::from(addr - 0x4000);
                match index {
                    0x14 => {
                        // OAMDMA is handled directly within the CPU so nothing left to do here
                    },
                    0x16 => {
                        // This register is split between being an APU register and a controller register
                        self.port1.write_register(data);
                        self.port2.write_register(data);
                        self.apu.write(addr, data);
                    },
                    0x17 => {
                        // This register is split between being an APU register and a controller register
                        self.apu.write(addr, data);
                    }
                    _ => {
                        self.apu.write(addr, data);
                    }
                }
            }
            _ => { // Cartridge
                self.cartridge.system_bus_write(addr, data);
                #[cfg(feature="ppu-sim")]
                self.ppu_sim_cartridge.system_bus_write(addr, data);
            }
        }

        //println!("Stepping system during CPU write");
        self.step_for_cpu_cycle();
    }

    /// Perform a system bus write from the CPU
    pub fn cpu_write(&mut self, addr: u16, data: u8) {
        self.check_watch_points(addr, WatchOps::WRITE);

        #[cfg(feature="io-stats")]
        {
            self.debug.io_stats[addr as usize].writes += 1;
        }

        self.write(addr, data);
    }

    /// Handle various superfluous writes that the CPU does
    ///
    /// Ideally we could discard these but they can have side effects so for now we only
    /// differentiate them from normal writes for debugging purposes.
    pub fn dummy_cpu_write(&mut self, addr: u16, data: u8) {
        self.check_watch_points(addr, WatchOps::DUMMY | WatchOps::WRITE);

        #[cfg(feature="io-stats")]
        {
            self.debug.io_stats[addr as usize].writes += 1;
        }

        self.write(addr, data);
    }

    #[cfg(feature="ppu-sim")]
    pub fn ppu_sim_step(&mut self, fb: *mut u8) -> PpuStatus {
        let sim_clks = self.ppu_sim.clk_per_pclk() * 2;
        let mut status = PpuStatus::None;
        for _ in 0..sim_clks {
            let new_status = self.ppu_sim.step_half(&mut self.cartridge, fb);
            if status == PpuStatus::None {
                status = new_status;
            }
        }
        status
    }

    //pub fn ppu_step(&mut self) {
    //    self.ppu.step(&mut self.cartridge);

        //self.step_ppu_sim(fb)
    //}
    pub fn ppu_clock(&self) -> u64 {
        self.ppu.clock
    }

    /// Returns: number of cycles to pause the CPU (form DMC sample buffer DMA)
    //pub fn apu_step(&mut self) -> Option<DmcDmaRequest> {
        //self.apu_clock += 1;
    //    self.apu.step()
    //}
    pub fn apu_clock(&self) -> u64 {
        self.apu.clock
    }

    // Single steps the PPU, and any side-car PPU simulator
    //
    // Returns false if the stepping was aborted due to hitting a PPU breakpoint
    #[inline(always)]
    fn step_ppu(&mut self) -> bool {
        if !self.ppu.step(&mut self.cartridge) {
            // PPU breakpoint hit
            return false;
        }

        // Record the cpu and ppu clock at the start of each line (before the first
        // PPU cycle for the line executes)
        //
        // NB: clocks are post-incremented so if we see dot == 0 then that cycle has
        // not yet actually elapsed.
        #[cfg(feature="trace-events")]
        if self.ppu.dot == 0 {
            let new_frame = self.ppu.line == 0;
            let cpu_clk = self.apu.clock; // NB: apu clock == cpu Clock
            self.ppu.trace_start_of_line(cpu_clk, new_frame);
            self.apu.trace_cpu_clock_line_sync(cpu_clk, new_frame);
            //self.cartridge.trace_cpu_clock_line_sync(cpu_clk);
        }

        #[cfg(feature="ppu-sim")]
        self.ppu_sim_step();

        true
    }

    /// Step everything connected to the system bus for a single CPU clock cycle
    ///
    /// It's guaranteed that this will be called once for each CPU cycle (usually as part of
    /// a `cpu_read` or cpu_write`) including cycles where it's "suspended" for OAMDMAs (where it's
    /// effectively just reading/writing on behalf of the PPU) or while halted for DMC DMA cycle stealing.
    ///
    /// Since the APU is clocked 1:1 by CPU clocks then it's enough to rely on this to clock
    /// the APU without any other mechanism to account for drift/divergence
    pub fn step_for_cpu_cycle(&mut self) {
        // The CPU should be checking for DMA requests after anything that might step
        // the system and the request should be taken / handled before stepping the
        // system again
        debug_assert!(self.dmc_dma_request.is_none());

        self.dmc_dma_request = self.apu.step();

        // There are always at least 3 pixel clocks per CPU cycle
        //
        // For PAL (3.2 pixel clocks) we will fall behind slightly within a single instruction
        // but that will be caught up in `Nes::progress()`. See `Self::catch_up_ppu_drift` below.
        //
        for _ in 0..3 {
            if !self.step_ppu() {
                // If we hit a PPU break point then we stop stepping the PPU
                // and we will catch up the PPU cycles before starting the next
                // CPU instruction
                break;
            }
        }
    }

    pub fn catch_up_ppu_drift(&mut self, expected_ppu_clock: u64) -> bool {
        //println!("cpu clock = {}, expected PPU clock = {}, actual ppu clock = {}", self.cpu.clock, expected_ppu_clock, self.system.ppu_clock());
        let ppu_delta = expected_ppu_clock - self.ppu.clock;

        for _ in 0..ppu_delta {
            if !self.step_ppu() {
                // Abort catch up if we hit a PPU dot breakpoint
                return false;
            }
        }

        true
    }

    pub fn add_watch(&mut self, addr: u16, ops: WatchOps) {
        if let Some(i) = self.debug.watch_points.iter().position(|w| w.address == addr) {
            self.debug.watch_points.swap_remove(i);
        }
        self.debug.watch_points.push(WatchPoint {
            address: addr,
            ops
        })
    }

    #[cfg(feature="trace-events")]
    #[inline(always)]
    pub fn trace(&mut self, event: TraceEvent) {
        self.ppu.trace(event)
    }

}