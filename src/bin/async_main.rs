#![no_std]
#![no_main]

use defmt::{debug, error, info};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::{rmt::Rmt, time::RateExtU32};
//use esp_hal::rsa::Rsa;
use embassy_net::{
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
    Runner, Stack,
};
use esp_hal_smartled::{smartLedBuffer, SmartLedsAdapter};
use esp_wifi::wifi::{WifiController, WifiDevice, WifiStaDevice};
use smart_leds::{
    brightness,
    colors,
    gamma,
    //hsv::{hsv2rgb, Hsv},
    SmartLedsWrite,
    RGB8,
};
use {defmt_rtt as _, esp_backtrace as _};

use reqwless::client::{HttpClient, TlsConfig};
use reqwless::request::RequestBuilder;

use anyhow::Result;
use semver::Version;

use bb_bot_weird::config;
use bb_bot_weird::error::BBBotError;
use botifactory_types::ReleaseBody;

extern crate alloc;
use alloc::format;
//use alloc::string::String;
use alloc::string::ToString;

use bb_bot_simplified_wifi::WifiManager;
use static_cell::StaticCell;

static WIFI_MANAGER: StaticCell<WifiManager> = StaticCell::new();

async fn check_for_fw_updates(stack: &'static Stack<'_>, tls_seed: u64) -> Result<()> {
    debug!("checking for fw updates");
    let release_url = format!(
        "{}/{}/{}/latest",
        config::BOTIFACTORY_URL,
        config::BOTIFACTORY_PROJECT_NAME,
        config::BOTIFACTORY_CHANNEL_NAME
    );
    debug!("release_url: {=str}", release_url.to_string());

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];
    debug!("socket/client setup");
    let dns_socket = DnsSocket::new(*stack);
    let tcp_state = TcpClientState::<1, 4096, 4096>::new();
    let tcp_client = TcpClient::new(*stack, &tcp_state);
    debug!("socket/client done");

    //let mut rsa = Rsa::new(peripherals.RSA);
    //let mut tcp_client = TcpClient::new(*stack, &state);
    let config = TlsConfig::new(
        tls_seed,
        &mut tx_buffer,
        &mut rx_buffer,
        reqwless::client::TlsVerify::None,
    );
    debug!("http client");
    let mut client = HttpClient::new_with_tls(&tcp_client, &dns_socket, config);

    let mut buffer = [0u8; 4096];
    debug!("building request");
    let headers = [("accept", "application/json")];
    let mut request = client
        .request(reqwless::request::Method::GET, &release_url)
        .await
        .map_err(BBBotError::from)?
        .content_type(reqwless::headers::ContentType::ApplicationJson)
        .headers(&headers);

    debug!("sending request");
    let response = request.send(&mut buffer).await.map_err(BBBotError::from)?;
    debug!("status code: {}", response.status);
    if response.status.is_successful() {
        debug!("reading response");
        let response_body = response
            .body()
            .read_to_end()
            .await
            .map_err(BBBotError::from)?;
        debug!("response read");

        let content = core::str::from_utf8(response_body)?;
        debug!("conent read");

        let (release_response, _size): (ReleaseBody, usize) =
            serde_json_core::from_str(content).map_err(BBBotError::from)?;
        let latest_version = release_response.release.version;
        let binary_version = Version::parse(config::RELEASE_VERSION).map_err(BBBotError::from)?;

        info!("latest version: {=str}", latest_version.to_string());
        info!("binary version: {=str}", binary_version.to_string());
    } else {
        error!("error response");
    }
    Ok(())
}

#[embassy_executor::task]
async fn check_for_fw_updates_task(stack: &'static Stack<'static>, tls_seed: u64) {
    loop {
        debug!("check_for_fw_updates_task called");
        match check_for_fw_updates(stack, tls_seed).await {
            Ok(_) => debug!("success!!"),
            Err(error) => error!("error checking for updates: {=str}", error.to_string()),
        }
        Timer::after(Duration::from_secs(10)).await;
        debug!("check_for_fw_updates done?");
    }
}

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // generator version: 0.2.2

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(72 * 1024);

    let timer0 = esp_hal::timer::systimer::SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);
    info!("Embassy initialized!");

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
