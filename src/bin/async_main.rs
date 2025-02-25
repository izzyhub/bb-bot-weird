#![no_std]
#![no_main]

use defmt::{info, debug, error, Format};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::{delay::Delay, main, rmt::Rmt, time::RateExtU32};
use esp_hal::rng::Rng;
use esp_hal::time;
use esp_hal::clock::CpuClock;
use esp_hal::rsa::Rsa;
use {defmt_rtt as _, esp_backtrace as _};
use esp_hal_smartled::{smartLedBuffer, SmartLedsAdapter};
use smart_leds::{
    brightness, gamma,
    hsv::{hsv2rgb, Hsv},
    SmartLedsWrite,
    RGB8,
    colors,
};
use embassy_net::{
    DhcpConfig,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
    Stack, StackResources, Runner,
};
use esp_wifi::{
    EspWifiController,
    init,
    wifi::{
        WifiController, WifiState, WifiEvent, WifiDevice,
        utils::create_network_interface, AccessPointInfo, AuthMethod, ClientConfiguration,
        Configuration, WifiError, WifiStaDevice,
    },
};
use smoltcp::{
    iface::{SocketSet, SocketStorage},
    wire::{DhcpOption, IpAddress},
};

use reqwless;
use reqwless::request::RequestBuilder;
use reqwless::client::{HttpClient, TlsConfig};

use anyhow::Result;
use semver::Version;

use botifactory_types::{ProjectBody, ReleaseBody};
use bb_bot_weird::config;
use bb_bot_weird::error::BBBotError;

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;

// When you are okay with using a nightly compiler it's better to use
// https://docs.rs/static_cell/2.1.0/static_cell/macro.make_static.html
macro_rules! mk_static {
        ($t:ty,$val:expr) => {{
                    static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
                    #[deny(unused_attributes)]
                    let x = STATIC_CELL.uninit().write(($val));
                    x
                }};
}

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
        .await.map_err(|error| BBBotError::from(error))?
        .content_type(reqwless::headers::ContentType::ApplicationJson)
        .headers(&headers);

    debug!("sending request");
    let mut response = request.send(&mut buffer)
        .await.map_err(|error| BBBotError::from(error))?;
    debug!("status code: {}", response.status);
    if response.status.is_successful() {
        debug!("reading response");
        let response_body = response.body().read_to_end().await
            .map_err(|error| BBBotError::from(error))?;
        debug!("response read");

        let content = core::str::from_utf8(response_body)?;
        debug!("conent read");

        let (release_response, size): (ReleaseBody, usize) = serde_json_core::from_str(content).map_err(|e| BBBotError::from(e))?;
        let latest_version = release_response.release.version;
        let binary_version = Version::parse(config::RELEASE_VERSION).map_err(|error| BBBotError::from(error))?;

        info!("latest version: {=str}", latest_version.to_string());
        info!("binary version: {=str}", binary_version.to_string());
    }
    else {
        error!("error response");
    }
    Ok(())

}

#[embassy_executor::task]
async fn check_for_fw_updates_task(stack: &'static Stack<'static>, tls_seed: u64)
{
    loop {
        debug!("check_for_fw_updates_task called");
        match check_for_fw_updates(stack, tls_seed).await {
            Ok(_) => debug!("success!!"),
            Err(error) => error!("error checking for updates: {=str}", error.to_string())
        }
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

    let mut rng = Rng::new(peripherals.RNG);

    let timer1 = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let wifi_init = &*mk_static!(EspWifiController<'static>,
        esp_wifi::init(
            timer1.timer0,
            rng,
            peripherals.RADIO_CLK,
        ).expect("expected to create init function for esp_wifi")
    );

    info!("Embassy initialized!");
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


    //let wifi = peripherals.WIFI;
    let (wifi_interface, controller) =
    esp_wifi::wifi::new_with_mode(&wifi_init, peripherals.WIFI, WifiStaDevice).unwrap();
    let dhcp_config = DhcpConfig::default();
    //dhcp_config.hostname = Some(String::from("bb-bot-weird"));

    let net_config = embassy_net::Config::dhcpv4(dhcp_config);

    let net_seed = rng.random() as u64 | ((rng.random() as u64) << 32);
    let tls_seed = rng.random() as u64 | ((rng.random() as u64) << 32);

    let (stack, runner) = mk_static!((Stack, Runner<WifiDevice<WifiStaDevice>>),
        embassy_net::new(
            wifi_interface,
            net_config,
            mk_static!(StackResources<3>, StackResources::<3>::new()),
            net_seed
    ));

    spawner.spawn(connection_task(controller)).ok();
    spawner.spawn(net_task(runner)).ok();
    stack.wait_config_up().await;

    //let mut rx_buffer = [0; 4096];
    //let mut tx_buffer = [0; 4096];

    info!("Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config_v4() {
            info!("Got IP: {}", config.address);
            debug!("debug logs?");
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    debug!("spawning check_for_fw_updates_task");
    //let rsa = Rsa::new(peripherals.RSA);
    loop {
        spawner.spawn(check_for_fw_updates_task(stack, tls_seed)).ok();
        Timer::after(Duration::from_secs(10)).await;
    }

    /*
    loop {
        //info!("Hello world!");
        //Timer::after(Duration::from_secs(1)).await;
        info!("Hello world!");
        Timer::after(Duration::from_secs(1)).await;
    }
    */

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/v0.23.1/examples/src/bin
}

async fn establish_wifi_connect(mut controller: WifiController<'static>) -> Result<()> {
    info!("start connection task");
    //info!("Device capabilities: {:#?}", controller.capabilities());
    loop {
        match esp_wifi::wifi::wifi_state() {
            WifiState::StaConnected => {
                // wait until we're no longer connected
                controller.wait_for_event(WifiEvent::StaDisconnected).await;
                Timer::after(Duration::from_millis(5000)).await
            }
            _ => { 
                debug!("weird state???");
            }
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = Configuration::Client(ClientConfiguration {
                ssid: config::WIFI_SSID.try_into().expect("WIFI_SSID not defined?"),
                password: config::WIFI_PASS.try_into().expect("WIFI_PASS not defined?"),
                ..Default::default()
            });
            controller.set_configuration(&client_config);
            info!("starting wifi");
            controller.start_async().await.expect("failed to start controller");
            info!("wifi started");
        }
        info!("about to connect...");
        match controller.connect_async().await {
            Ok(_) => info!("Wifi connected"),
            Err(err) => {
                error!("failed to connect to wifi: {}", err);
                Timer::after(Duration::from_millis(5000)).await;
            }
        }
    }

}
#[embassy_executor::task]
async fn net_task(runner: &'static mut Runner<'static, WifiDevice<'static, WifiStaDevice>>) {
    runner.run().await
}
#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) {
    establish_wifi_connect(controller).await;
}
