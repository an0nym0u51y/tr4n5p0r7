/**************************************************************************************************
 *                                                                                                *
 * This Source Code Form is subject to the terms of the Mozilla Public                            *
 * License, v. 2.0. If a copy of the MPL was not distributed with this                            *
 * file, You can obtain one at http://mozilla.org/MPL/2.0/.                                       *
 *                                                                                                *
 **************************************************************************************************/

// =========================================== Imports ========================================== \\

pub use protocol::{self, packets};

use async_channel as channel;
use async_oneshot as oneshot;
use async_io::Timer;
use async_peek::AsyncPeek;
use async_tcp::{TcpListener, TcpStream};
use core::time::Duration;
use futures_lite::{AsyncRead, AsyncWrite, FutureExt};
use protocol::{Handshake, Packet, Protocol};
use protocol::packets::PacketId;
use std::io;
use std::net::SocketAddr;

#[cfg(feature = "thiserror")]
use thiserror::Error;

// ============================================ Types =========================================== \\

pub struct Listener<Inner> {
    config: Config,
    inner: Inner,
}

pub struct Connection<Inner> {
    config: Config,
    channels: Channels,
    proto: Protocol,
    inner: Inner,
}

#[derive(Clone)]
pub struct Config {
    pub max_heartbeats: usize,
    pub heartbeat_rate: Duration,
}

pub struct Channels {
    pub close: Option<oneshot::Receiver<()>>,
    pub recved: channel::Sender<Packet>,
    pub send: channel::Receiver<Packet>,
    pub sending: channel::Sender<(PacketId, oneshot::Sender<Packet>)>,
}

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug)]
#[cfg_attr(feature = "thiserror", derive(Error))]
pub enum Error {
    #[cfg_attr(feature = "thiserror", error("connection closed"))]
    Closed,
    #[cfg_attr(feature = "thiserror", error(""))]
    Hello,
    #[cfg_attr(feature = "thiserror", error(""))]
    Io(io::Error),
    #[cfg_attr(feature = "thiserror", error(""))]
    Pr070c01(protocol::Error),
}

// ======================================== impl Listener ======================================= \\

impl<Inner: TcpListener> Listener<Inner> {
    // ==================================== Constructors ==================================== \\

    pub async fn bind(addr: SocketAddr, config: Config) -> Result<Self> {
        Ok(Listener {
            config,
            inner: Inner::bind(addr).await?,
        })
    }

    // ===================================== Read+Write ===================================== \\

    pub async fn accept(&self, channels: Channels) -> Result<Connection<Inner::Stream>>
    where
        for<'stream> &'stream Inner::Stream: AsyncPeek + AsyncRead + AsyncWrite,
    {
        let (stream, _) = self.inner.accept().await?;
        let handshake = Handshake::respond(&stream, &stream).await?;
        let mut conn = Connection {
            config: self.config.clone(),
            channels,
            proto: handshake.done(),
            inner: stream,
        };

        // TODO: timeout
        if let packet @ Packet::Hello(_) = conn.recv().await? {
            conn.channels.recved.send(packet).await.map_err(|_| Error::Hello)?;
        } else {
            todo!()
        }

        let (sender, recver) = oneshot::oneshot();
        conn.channels.sending.send((PacketId::Hello, sender)).await?;
        if let packet @ Packet::Hello(_) = recver.await.map_err(|_| Error::Hello)? {
            conn.send(packet).await?;
        } else {
            todo!()
        }

        Ok(conn)
    }
}

// ======================================= impl Connection ====================================== \\

impl<Inner: TcpStream> Connection<Inner>
where
    for<'inner> &'inner Inner: AsyncPeek + AsyncRead + AsyncWrite,
{
    // ==================================== Constructors ==================================== \\

    pub async fn connect(addr: SocketAddr, config: Config, channels: Channels) -> Result<Self> {
        let stream = Inner::connect(addr).await?;
        let handshake = Handshake::initiate(&stream, &stream).await?;
        let mut conn = Connection {
            config,
            channels,
            proto: handshake.done(),
            inner: stream,
        };

        let (sender, recver) = oneshot::oneshot();
        conn.channels.sending.send((PacketId::Hello, sender)).await?;
        if let packet @ Packet::Hello(_) = recver.await.map_err(|_| Error::Hello)? {
            conn.send(packet).await?;
        } else {
            todo!()
        }

        // TODO: timeout
        if let packet @ Packet::Hello(_) = conn.recv().await? {
            conn.channels.recved.send(packet).await.map_err(|_| Error::Hello)?;
        } else {
            todo!()
        }

        Ok(conn)
    }

    // ===================================== Destructors ==================================== \\

    pub async fn run(mut self) -> Result<()> {
        let mut close = self.channels.close.take().unwrap();
        let mut heartbeat = Timer::after(self.config.heartbeat_rate);
        let mut heartbeats = 0;

        loop {
            match async {
                let _ = (&mut close).await;
                Err(Error::Closed)
            }.or(async {
                (&mut heartbeat).await;
                Ok(None)
            }).or(async {
                let packet = self.recv().await?;
                Ok(Some(packet))
            }).await? {
                Some(Packet::Heartbeat(_)) => heartbeats = 0,
                Some(Packet::Hello(_)) => todo!(),
                None if heartbeats > self.config.max_heartbeats => todo!(),
                None => {
                    self.send(Packet::heartbeat()).await?;

                    heartbeat.set_after(self.config.heartbeat_rate);
                    heartbeats += 1;
                }
            }
        }
    }

    // ===================================== Read+Write ===================================== \\

    async fn send(&mut self, packet: Packet) -> Result<usize> {
        Ok(self.proto.send(&self.inner, packet).await?)
    }

    async fn recv(&mut self) -> Result<Packet> {
        Ok(self.proto.recv(&self.inner).await?)
    }
}

// ========================================== impl From ========================================= \\

impl From<channel::SendError<packets::Hello>> for Error {
    #[inline]
    fn from(_: channel::SendError<packets::Hello>) -> Self {
        Error::Hello
    }
}

impl From<channel::SendError<(PacketId, oneshot::Sender<Packet>)>> for Error {
    #[inline]
    fn from(error: channel::SendError<(PacketId, oneshot::Sender<Packet>)>) -> Self {
        match error.into_inner().0 {
            PacketId::Heartbeat => todo!(),
            PacketId::Hello => Error::Hello,
        }
    }
}

impl From<io::Error> for Error {
    #[inline]
    fn from(error: io::Error) -> Self {
        Error::Io(error)
    }
}

impl From<protocol::Error> for Error {
    #[inline]
    fn from(error: protocol::Error) -> Self {
        Error::Pr070c01(error)
    }
}
