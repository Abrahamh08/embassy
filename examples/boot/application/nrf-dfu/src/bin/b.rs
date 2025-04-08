#![no_std]
#![no_main]
#![macro_use]

use core::cell::RefCell;

#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::{
    Peri,
    peripherals::USBD,
    usb::{Driver, vbus_detect::HardwareVbusDetect},
    nvmc::Nvmc,
};
use embassy_sync::blocking_mutex::{Mutex, raw::NoopRawMutex};
use embassy_time::Duration;
use embassy_usb::UsbDevice;
use embassy_usb_dfu::consts::DfuAttributes;
use embassy_usb_dfu::{usb_dfu, Control, ResetImmediate};
use static_cell::{ConstStaticCell, StaticCell};
use panic_reset as _;

pub const VID: u16 = 0xc0de;
pub const PID: u16 = 0xcafe;

embassy_nrf::bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_nrf::init(Default::default());

    // let mut _led = Output::new(p.P0_13, Level::Low, OutputDrive::Standard);
    let mut _led = Output::new(p.P0_31, Level::Low, OutputDrive::Standard);

    //let mut _led = Output::new(p.P1_10, Level::Low, OutputDrive::Standard);

    // nRF91 DK
    // let mut _led = Output::new(p.P0_02, Level::Low, OutputDrive::Standard);

    static NVMC: static_cell::StaticCell<Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc>>> = static_cell::StaticCell::new();
    let nvmc = NVMC.init_with(|| Mutex::new(RefCell::new(Nvmc::new(p.NVMC))));
    let nvmc = &*nvmc;

    let usb = usb_device(p.USBD, &nvmc);
    spawner.must_spawn(run_usb(usb));

    loop {
        _led.set_high();
        embassy_time::Timer::after_secs(1).await;

        _led.set_low();
        embassy_time::Timer::after_secs(1).await;
    }
}

fn device_id() -> [u8; 6] {
    let ficr = embassy_nrf::chip::pac::FICR;
    let low = ficr.deviceid(0).read();
    let high = ficr.deviceid(1).read();
    let [a, b, c, d] = low.to_le_bytes();
    let [e, f, ..] = high.to_le_bytes();
    [a, b, c, d, e, f]
}

fn device_id_str(buf: &mut [u8; 16]) -> &str {
    const CHARACTERS: [u8; 16] = *b"0123456789ABCDEF";
    let id = device_id();
    for (a, b) in id.into_iter().zip(buf.chunks_mut(2)) {
        b[0] = CHARACTERS[(a >> 4) as usize];
        b[1] = CHARACTERS[(a % 16) as usize];
    }
    unsafe { core::str::from_utf8_unchecked(buf) }
}

type StaticUsbDevice = UsbDevice<'static, Driver<'static, USBD, HardwareVbusDetect>>;

#[embassy_executor::task]
pub async fn run_usb(mut device: StaticUsbDevice) -> ! {
    device.run().await
}

/// Panics if called more than once.
pub fn usb_device(p: Peri<'static, USBD>, flash: &'static Mutex<NoopRawMutex, RefCell<Nvmc<'_>>>) -> StaticUsbDevice {
    // Create the driver, from the HAL.
    let driver = Driver::new(p, Irqs, HardwareVbusDetect::new(Irqs));

    // Create embassy-usb Config
    let mut config = embassy_usb::Config::new(VID, PID);
    static SERIAL_NUMBER_BUFFER: ConstStaticCell<[u8; 16]> = ConstStaticCell::new([0; 16]);
    config.manufacturer = Some("Nordic Semiconductor");
    config.product = Some("USB DFU Sample");
    config.serial_number = Some(device_id_str(SERIAL_NUMBER_BUFFER.take()));
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    // Create embassy-usb DeviceBuilder using the driver and config.
    // It needs some buffers for building the descriptors.
    static CONFIG_DESCRIPTOR: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
    static BOS_DESCRIPTOR: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
    static MSOS_DESCRIPTOR: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
    static CONTROL_BUF: ConstStaticCell<[u8; 64]> = ConstStaticCell::new([0; 64]);

    let config_descriptor = CONFIG_DESCRIPTOR.take();
    let bos_descriptor = BOS_DESCRIPTOR.take();
    let msos_descriptor = MSOS_DESCRIPTOR.take();
    let control_buf = CONTROL_BUF.take();

    let mut builder = embassy_usb::Builder::new(
        driver,
        config,
        config_descriptor,
        bos_descriptor,
        msos_descriptor,
        control_buf,
    );

    let config = embassy_boot::FirmwareUpdaterConfig::from_linkerfile_blocking(flash, flash);

    static MAGIC: StaticCell<embassy_boot::AlignedBuffer<4>> = StaticCell::new();

    let magic = MAGIC.init_with(|| embassy_boot::AlignedBuffer([0; 4usize]));

    let mut firmware_state = embassy_boot::BlockingFirmwareState::from_config(config, &mut magic.0);
    firmware_state.mark_booted().expect("Failed to mark booted");

    static FIRMWARE_STATE: StaticCell<embassy_usb_dfu::Control<'_, embassy_embedded_hal::flash::partition::BlockingPartition<'_, NoopRawMutex, Nvmc>, ResetImmediate>> = StaticCell::new();
    let state = FIRMWARE_STATE.init_with(|| Control::new(firmware_state, DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH | DfuAttributes::MANIFESTATION_TOLERANT));
    usb_dfu::<_, _, ResetImmediate>(&mut builder, state, Duration::from_millis(2500));

    // Build the builder.
    let usb = builder.build();
    usb
}
