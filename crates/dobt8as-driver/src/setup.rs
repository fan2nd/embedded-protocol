use crate::{Command, Driver, Error, Event, Field, Reply, Transport, BAUDS};
#[derive(Debug, PartialEq, Eq)]
pub enum SetupError<E> {
    Driver(Error),
    Transport(E),
    Timeout,
    Device(u16),
    NotFound,
    Readback(Field),
}
pub struct Options<'a> {
    pub name: &'a str,
    pub baud: u32,
    pub reset_wait_ms: u32,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Ready {
    pub detected_baud: u32,
    pub baud: u32,
    pub connected_at_commands_disabled: bool,
    pub logs_disabled: bool,
}
impl Driver {
    pub async fn execute<T: Transport>(
        &mut self,
        io: &mut T,
        cmd: Command<'_>,
    ) -> Result<Reply, SetupError<T::Error>> {
        if matches!(cmd, Command::SetBaud(_)) {
            return Err(SetupError::Driver(Error::UseChangeBaud));
        }
        let mut tx = [0; 64];
        let n = self.begin(cmd, &mut tx).map_err(SetupError::Driver)?;
        let result = async {
            io.drain(20, 4096).await.map_err(SetupError::Transport)?;
            let start = io.now_ms();
            io.write(&tx[..n], 800)
                .await
                .map_err(SetupError::Transport)?;
            let mut rx = [0; 128];
            loop {
                let elapsed = io.now_ms().saturating_sub(start);
                if elapsed >= 800 {
                    return Err(SetupError::Timeout);
                }
                let n = io
                    .read(&mut rx, (800 - elapsed) as u32)
                    .await
                    .map_err(SetupError::Transport)?;
                if n == 0 {
                    return Err(SetupError::Timeout);
                }
                let mut result = None;
                // Consume the whole received block, including URCs following OK.
                for &b in &rx[..n] {
                    match self.feed(b) {
                        Some(Event::Complete(r)) => result = Some(Ok(r)),
                        Some(Event::DeviceError(code)) if result.is_none() => {
                            result = Some(Err(SetupError::Device(code)))
                        }
                        Some(Event::ParseError(e)) if result.is_none() => {
                            result = Some(Err(SetupError::Driver(e)))
                        }
                        _ => {}
                    }
                }
                if let Some(result) = result {
                    return result;
                }
            }
        }
        .await;
        if result.is_err() {
            self.cancel();
        }
        result
    }
    /// BAUD changes immediately. Never wait for an OK at the old UART rate.
    /// Success requires a fresh BAUD query at the new rate.
    pub async fn change_baud<T: Transport>(
        &mut self,
        io: &mut T,
        baud: u32,
    ) -> Result<(), SetupError<T::Error>> {
        let mut tx = [0; 64];
        let n = self
            .begin(Command::SetBaud(baud), &mut tx)
            .map_err(SetupError::Driver)?;
        let result = async {
            io.drain(20, 4096).await.map_err(SetupError::Transport)?;
            io.write(&tx[..n], 800)
                .await
                .map_err(SetupError::Transport)?;
            self.cancel();
            io.set_baud(baud).await.map_err(SetupError::Transport)?;
            if self
                .execute(io, Command::Query(Field::Baud))
                .await?
                .number()
                != Some(baud)
            {
                return Err(SetupError::Readback(Field::Baud));
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            self.cancel();
        }
        result
    }
    /// Prepare a disconnected slave for binary transport. Does not change UUID or
    /// issue factory reset, and does not claim that a BLE connection is established.
    pub async fn configure<T: Transport>(
        &mut self,
        io: &mut T,
        o: Options<'_>,
    ) -> Result<Ready, SetupError<T::Error>> {
        let mut validation = Driver::new();
        validation
            .begin(Command::SetName(o.name), &mut [0; 64])
            .map_err(SetupError::Driver)?;
        validation.cancel();
        validation
            .begin(Command::SetBaud(o.baud), &mut [0; 64])
            .map_err(SetupError::Driver)?;
        if self.connected {
            return Err(SetupError::Driver(Error::NotReady));
        }
        self.begin(Command::Query(Field::Baud), &mut [0; 64])
            .map_err(SetupError::Driver)?;
        self.cancel();
        let mut detected = None;
        'scan: for baud in BAUDS {
            io.set_baud(baud).await.map_err(SetupError::Transport)?;
            // Low-power firmware may miss UART bytes; use bounded retries.
            for _ in 0..3 {
                match self.execute(io, Command::Query(Field::Baud)).await {
                    Ok(r) if r.number() == Some(baud) => {
                        detected = Some(baud);
                        break 'scan;
                    }
                    Ok(_) | Err(SetupError::Timeout) | Err(SetupError::Device(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        let detected_baud = detected.ok_or(SetupError::NotFound)?;
        if detected_baud != o.baud {
            self.change_baud(io, o.baud).await?;
        }
        let name = self.execute(io, Command::Query(Field::Name)).await?;
        if name.value() != o.name.as_bytes() {
            self.execute(io, Command::SetName(o.name)).await?;
        }
        self.execute(io, Command::SetLog(false)).await?;
        // UART reception may lose bytes in low power. Bounded repeated attempts
        // follow the manual's suggestion, followed by mandatory readback.
        let mut awake = false;
        for _ in 0..3 {
            match self.execute(io, Command::SetLowPower(false)).await {
                Ok(r) if r.number() == Some(0) => {
                    awake = true;
                    break;
                }
                Ok(_) | Err(SetupError::Timeout) => {}
                Err(e) => return Err(e),
            }
        }
        if !awake {
            return Err(SetupError::Readback(Field::LowPower));
        }
        match self.execute(io, Command::Reset).await {
            Ok(_) | Err(SetupError::Timeout) => {}
            Err(e) => return Err(e),
        }
        io.delay_ms(o.reset_wait_ms).await;
        self.disconnected();
        self.at_enabled = None;
        if self
            .execute(io, Command::Query(Field::Baud))
            .await?
            .number()
            != Some(o.baud)
        {
            return Err(SetupError::Readback(Field::Baud));
        }
        if self.execute(io, Command::Query(Field::Name)).await?.value() != o.name.as_bytes() {
            return Err(SetupError::Readback(Field::Name));
        }
        if self.execute(io, Command::Query(Field::Log)).await?.number() != Some(0) {
            return Err(SetupError::Readback(Field::Log));
        }
        self.execute(io, Command::SetLowPower(false)).await?;
        if self
            .execute(io, Command::Query(Field::LowPower))
            .await?
            .number()
            != Some(0)
        {
            return Err(SetupError::Readback(Field::LowPower));
        }
        self.execute(io, Command::SetCommandRecognition(false))
            .await?;
        if self
            .execute(io, Command::Query(Field::CommandRecognition))
            .await?
            .number()
            != Some(0)
        {
            return Err(SetupError::Readback(Field::CommandRecognition));
        }
        Ok(Ready {
            detected_baud,
            baud: o.baud,
            connected_at_commands_disabled: true,
            logs_disabled: true,
        })
    }
}
