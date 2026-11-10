//! Board/application AT setup policy; not part of the reusable device driver.
use dobt01_driver::{transaction::TransactionError, Command, Driver, Error, Response};
use embassy_stm32::usart::{self, BufferedUart, Config};
use embassy_time::{with_timeout, Duration, Timer};
use embedded_io_async::Read;
const SCAN_BAUDS: [u32; 13] = [
    115200, 9600, 230400, 256000, 460800, 512000, 1000000, 57600, 38400, 19200, 31250, 2400, 4800,
];
#[allow(dead_code)]
#[derive(Debug)]
pub enum SetupError {
    Driver(Error),
    Transport(usart::Error),
    Transaction(TransactionError<usart::Error>),
    Timeout,
    NotFound,
    Unexpected(Response),
    BaudReadback,
    NameReadback,
    Config,
}
pub struct Options<'a> {
    pub name: &'a str,
    pub baud: u32,
    pub ensure_slave: bool,
}
pub struct Ready {
    pub detected_baud: u32,
    pub baud: u32,
    pub role: Option<u8>,
    pub connection: Option<u8>,
}
fn set_baud(io: &mut BufferedUart<'_>, baud: u32) -> Result<(), SetupError> {
    let mut cfg = Config::default();
    cfg.baudrate = baud;
    cfg.eager_reads = Some(64);
    defmt::info!("AT UART baud={}", baud);
    io.set_config(&cfg).map_err(|_| SetupError::Config)
}
async fn drain(io: &mut BufferedUart<'_>) -> Result<(), SetupError> {
    let mut buf = [0; 128];
    let mut total = 0;
    while total < 4096 {
        match with_timeout(Duration::from_millis(20), io.read(&mut buf)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => total += n,
            Ok(Err(e)) => return Err(SetupError::Transport(e)),
        }
    }
    Ok(())
}
async fn execute(
    driver: &mut Driver,
    io: &mut BufferedUart<'_>,
    cmd: Command,
) -> Result<Response, SetupError> {
    drain(io).await?;
    with_timeout(Duration::from_millis(800), driver.execute(io, cmd))
        .await
        .map_err(|_| SetupError::Timeout)?
        .map_err(SetupError::Transaction)
}
async fn set_name(
    driver: &mut Driver,
    io: &mut BufferedUart<'_>,
    name: &str,
) -> Result<Response, SetupError> {
    drain(io).await?;
    with_timeout(Duration::from_millis(800), driver.set_name(io, name))
        .await
        .map_err(|_| SetupError::Timeout)?
        .map_err(SetupError::Transaction)
}
/// Scan, configure, reset and verify; does not claim the BLE peer is connected.
/// Call only while the application exclusively owns a quiescent AT connection.
pub async fn configure(
    driver: &mut Driver,
    io: &mut BufferedUart<'_>,
    options: Options<'_>,
) -> Result<Ready, SetupError> {
    // Validate configuration before any I/O; preserve the mode/busy gate.
    let mut validation = Driver::new();
    validation
        .begin(Command::SetBaud(options.baud), &mut [0; 32])
        .map_err(SetupError::Driver)?;
    validation.cancel();
    validation
        .begin_name(options.name, &mut [0; 32])
        .map_err(SetupError::Driver)?;
    driver
        .begin(Command::Baud, &mut [0; 32])
        .map_err(SetupError::Driver)?;
    driver.cancel();
    Timer::after_millis(1000).await;
    let mut found = None;
    for baud in SCAN_BAUDS {
        set_baud(io, baud)?;
        match execute(driver, io, Command::Baud).await {
            Ok(Response::Baud(reported)) if reported == baud => {
                found = Some(baud);
                break;
            }
            Ok(_) | Err(SetupError::Timeout) => {}
            Err(e) => return Err(e),
        }
    }
    let detected_baud = found.ok_or(SetupError::NotFound)?;
    let mut role = match execute(driver, io, Command::Role).await? {
        Response::Role(v) => Some(v),
        Response::ErrorCode(2) => None,
        r => return Err(SetupError::Unexpected(r)),
    };
    if options.ensure_slave && role == Some(1) {
        expect_ok(execute(driver, io, Command::Slave).await?)?;
        Timer::after_millis(2000).await;
        role = match execute(driver, io, Command::Role).await? {
            Response::Role(0) => Some(0),
            r => return Err(SetupError::Unexpected(r)),
        };
    }
    let named = execute(driver, io, Command::BleName).await? == Response::BleName
        && name_matches(driver, options.name);
    if !named {
        expect_ok(set_name(driver, io, options.name).await?)?;
    }
    if options.baud != detected_baud {
        expect_ok(execute(driver, io, Command::SetBaud(options.baud)).await?)?;
    }
    Timer::after_millis(1000).await;
    match execute(driver, io, Command::Reset).await {
        Ok(Response::Ok) | Err(SetupError::Timeout) => {}
        Ok(r) => return Err(SetupError::Unexpected(r)),
        Err(e) => return Err(e),
    }
    set_baud(io, options.baud)?;
    Timer::after_millis(2000).await;
    driver.disconnected(); // Module reset was requested; readback below is mandatory.
    if execute(driver, io, Command::Baud).await? != Response::Baud(options.baud) {
        return Err(SetupError::BaudReadback);
    }
    if execute(driver, io, Command::BleName).await? != Response::BleName
        || !name_matches(driver, options.name)
    {
        return Err(SetupError::NameReadback);
    }
    let connection = match execute(driver, io, Command::Connection).await? {
        Response::Connection(v) => Some(v),
        Response::ErrorCode(2) => None,
        r => return Err(SetupError::Unexpected(r)),
    };
    Ok(Ready {
        detected_baud,
        baud: options.baud,
        role,
        connection,
    })
}
fn name_matches(driver: &Driver, name: &str) -> bool {
    driver.line().strip_prefix(b"TM+") == Some(name.as_bytes())
}
fn expect_ok(r: Response) -> Result<(), SetupError> {
    if r == Response::Ok {
        Ok(())
    } else {
        Err(SetupError::Unexpected(r))
    }
}
