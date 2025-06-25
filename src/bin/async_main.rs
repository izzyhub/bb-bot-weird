#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_net::{
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
    Runner, Stack,
};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::reset;
use esp_hal::rng::Rng;
use esp_hal::{rmt::Rmt, time::RateExtU32};
use esp_hal_smartled::{smartLedBuffer, SmartLedsAdapter};
use esp_wifi::wifi::{WifiController, WifiDevice, WifiStaDevice};
use log::{debug, error, info};
use smart_leds::{
    brightness,
    colors,
    gamma,
    //hsv::{hsv2rgb, Hsv},
    SmartLedsWrite,
    RGB8,
};

use reqwless::client::{HttpClient, TlsConfig};

use anyhow::Result;
use semver::Version;

use bb_bot_weird::config;
use bb_bot_weird::error::BBBotError;
use esp_storage::FlashStorage;

extern crate alloc;

use bb_bot_simplified_wifi::WifiManager;
use botifactory_ota_nostd::{accept_fw, BotifactoryClient, BotifactoryUrlBuilder};
use esp_println::println;
use static_cell::StaticCell;

static WIFI_MANAGER: StaticCell<WifiManager> = StaticCell::new();
static TCP_STATE: StaticCell<TcpClientState<1, 4096, 4096>> = StaticCell::new();
static RX_BUFFER: StaticCell<[u8; 4096]> = StaticCell::new();
static TX_BUFFER: StaticCell<[u8; 4096]> = StaticCell::new();

async fn check_for_fw_updates(
    stack: &'static Stack<'_>,
    tcp_client: &TcpClient<'_, 1, 4096, 4096>,
    tls_seed: u64,
) -> Result<()> {
    let dns_socket = DnsSocket::new(*stack);

    let tx_buffer = TX_BUFFER.init([0; 4096]);
    let rx_buffer = RX_BUFFER.init([0; 4096]);
    let config = TlsConfig::new(
        tls_seed,
        tx_buffer,
        rx_buffer,
        reqwless::client::TlsVerify::None,
    );
    let client = HttpClient::new_with_tls(tcp_client, &dns_socket, config);

    let botifactory_url_builder = BotifactoryUrlBuilder::new(
        config::BOTIFACTORY_URL,
        config::BOTIFACTORY_PROJECT_NAME,
        config::BOTIFACTORY_CHANNEL_NAME,
    );
    let latest_url = botifactory_url_builder.latest();
    debug!("latest_url: {latest_url}");

    let mut botifactory_client = BotifactoryClient::new(latest_url, client);

    debug!("reading server version");
    let latest_version = botifactory_client.read_version().await?;
    let binary_version = Version::parse(config::RELEASE_VERSION).map_err(BBBotError::from)?;

    info!("latest version: {}", latest_version);
    info!("binary version: {}", binary_version);

    if latest_version > binary_version {
        let mut storage = FlashStorage::new();
        botifactory_client.read_binary(&mut storage).await?;
        info!("resetting");
        reset::software_reset();
    }

    Ok(())
}

#[embassy_executor::task]
async fn check_for_fw_updates_task(stack: &'static Stack<'static>, tls_seed: u64) {
    let tcp_state = TCP_STATE.init(TcpClientState::<1, 4096, 4096>::new());
    let tcp_client = TcpClient::new(*stack, tcp_state);

    loop {
        debug!("check_for_fw_updates_task called");
        match check_for_fw_updates(stack, &tcp_client, tls_seed).await {
            Ok(_) => debug!("success!!"),
            Err(error) => error!("error checking for updates: {}", error),
        }
        Timer::after(Duration::from_secs(300)).await;
        debug!("check_for_fw_updates done?");
    }
}

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // generator version: 0.2.2
    let binary_version = Version::parse(config::RELEASE_VERSION)
        .map_err(BBBotError::from)
        .expect("Should be a valid version number");
    info!("hello from version: {}", binary_version);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(72 * 1024);

    // Initialize simple logger that outputs to esp-println
    struct SimpleLogger;
    impl log::Log for SimpleLogger {
        fn enabled(&self, _metadata: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                esp_println::println!("[{}] {}", record.level(), record.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: SimpleLogger = SimpleLogger;
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Debug);

    let timer0 = esp_hal::timer::systimer::SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);
    println!("Starting application...");
    esp_println::println!("Direct esp_println test");
    info!("Embassy initialized!");
    info!("After info log");
    esp_println::println!("Another direct test");

    let mut rng = Rng::new(peripherals.RNG);

    let timer1 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);

    let wifi_manager = WIFI_MANAGER.init(
        WifiManager::new(rng, timer1.timer0, peripherals.RADIO_CLK, peripherals.WIFI)
            .await
            .expect("Failed to create wifi manager"),
    );

    let led_pin = peripherals.GPIO48;
    let freq = 80.MHz();
    let rmt = Rmt::new(peripherals.RMT, freq).unwrap();
    const LED_COUNT: usize = 1;
    let rmt_buffer = smartLedBuffer!(1);
    info!("created buffer??");
    let mut led = SmartLedsAdapter::new(rmt.channel0, led_pin, rmt_buffer);
    info!("created adapter");
    //let data: [RGB8; LED_COUNT] = [colors::WHITE_SMOKE];
    let data: [RGB8; LED_COUNT] = [colors::CHARTREUSE];
    //let data: [RGB8; LED_COUNT] = [colors::CORNSILK];
    led.write(brightness(gamma(data.iter().cloned()), 10))
        .unwrap();

    let data: [RGB8; LED_COUNT] = [colors::GREEN];
    led.write(brightness(gamma(data.iter().cloned()), 10))
        .unwrap();

    let tls_seed = rng.random() as u64 | ((rng.random() as u64) << 32);

    spawner.spawn(connection_task(wifi_manager.controller)).ok();
    spawner.spawn(net_task(wifi_manager.runner)).ok();
    wifi_manager.stack.wait_config_up().await;

    info!("Waiting to get IP address...");
    loop {
        if let Some(config) = wifi_manager.stack.config_v4() {
            info!("Got IP: {}", config.address.address());
            debug!("debug logs?");
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    let mut storage = FlashStorage::new();
    debug!("This version is slightly newer!!");
    if let Err(_error) = accept_fw(&mut storage) {
        error!("Error accepting firmware");
    }
    debug!("spawning check_for_fw_updates_task");
    spawner
        .spawn(check_for_fw_updates_task(wifi_manager.stack, tls_seed))
        .ok();
}

#[embassy_executor::task]
async fn net_task(runner: &'static mut Runner<'static, WifiDevice<'static, WifiStaDevice>>) {
    runner.run().await
}
#[embassy_executor::task]
async fn connection_task(controller: &'static mut WifiController<'static>) {
    match WifiManager::connect(
        controller,
        config::WIFI_SSID,
        config::WIFI_PASS,
        Duration::from_secs(10),
    )
    .await
    {
        Ok(_) => {
            debug!("connection successful");
        }
        Err(error) => {
            error!("Error connecting: {}", error)
        }
    }
}
