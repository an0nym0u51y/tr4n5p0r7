/**************************************************************************************************
 *                                                                                                *
 * This Source Code Form is subject to the terms of the Mozilla Public                            *
 * License, v. 2.0. If a copy of the MPL was not distributed with this                            *
 * file, You can obtain one at http://mozilla.org/MPL/2.0/.                                       *
 *                                                                                                *
 **************************************************************************************************/

// =========================================== Imports ========================================== \\

use async_channel as channel;
use async_oneshot as oneshot;
use async_peek::AsyncPeek;
use async_tcp::{TcpListener, TcpStream};
use futures_lite::{AsyncRead, AsyncWrite, FutureExt};
use protocol::{self, Handshake, Protocol};
use protocol::packets::{self, PacketId};
use std::io;
use std::net::SocketAddr;

// ============================================ Types =========================================== \\

pub struct Listener<Inner> {
    inner: Inner,
}

pub struct Connection<Inner> {
    channels: Channels,
    proto: Protocol,
    inner: Inner,
}

pub struct Channels {
    close: Option<oneshot::Receiver<()>>,
    hello_in: channel::Sender<packets::Hello>,
    hello_out: channel::Sender<oneshot::Sender<packets::Hello>>,
}

pub type Result<T> = core::result::Result<T, Error>;

pub enum Error {
    Closed,
    Hello,
    Io(io::Error),
    Pr070c01(protocol::Error),
}

// ======================================== impl Listener ======================================= \\

impl<Inner: TcpListener> Listener<Inner> {
    // ==================================== Constructors ==================================== \\

    pub async fn bind(addr: SocketAddr) -> Result<Self> {
        Ok(Listener {
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
            channels,
            proto: handshake.done(),
            inner: stream,
        };

        // TODO: timeout
        let hello = conn.try_recv::<packets::Hello>().await?;
        conn.channels.hello_in.send(hello).await?;

        let (sender, recver) = oneshot::oneshot();
        conn.channels.hello_out.send(sender).await?;
        let hello = recver.await.map_err(|_| Error::Hello)?;
        conn.send(hello).await?;

        Ok(conn)
    }
}

// ======================================= impl Connection ====================================== \\

impl<Inner: TcpStream> Connection<Inner>
where
    for<'inner> &'inner Inner: AsyncPeek + AsyncRead + AsyncWrite,
{
    // ==================================== Constructors ==================================== \\

    pub async fn connect(addr: SocketAddr, channels: Channels) -> Result<Self> {
        let stream = Inner::connect(addr).await?;
        let handshake = Handshake::initiate(&stream, &stream).await?;
        let mut conn = Connection {
            channels,
            proto: handshake.done(),
            inner: stream,
        };

        let (sender, recver) = oneshot::oneshot();
        conn.channels.hello_out.send(sender).await?;
        let hello = recver.await.map_err(|_| Error::Hello)?;
        conn.send(hello).await?;

        // TODO: timeout
        let hello = conn.try_recv::<packets::Hello>().await?;
        conn.channels.hello_in.send(hello).await?;

        Ok(conn)
    }

    // ===================================== Destructors ==================================== \\

    pub async fn run(mut self) -> Result<()> {
        let mut close = self.channels.close.take().unwrap();

        loop {
            match async {
                let _ = (&mut close).await;
                Err(Error::Closed)
            }.or(self.peek_packet_id()).await? {
                PacketId::Heartbeat => todo!(),
                PacketId::Hello => {
                    let hello = self.try_recv::<packets::Hello>().await?;
                    self.channels.hello_in.send(hello).await?;

                    todo!()
                },
            }
        }
    }

    // ===================================== Read+Write ===================================== \\

    async fn send<Packet: packets::Packet>(&mut self, packet: Packet) -> Result<usize> {
        Ok(self.proto.send(&self.inner, packet).await?)
    }

    async fn try_recv<Packet: packets::Packet>(&mut self) -> Result<Packet> {
        Ok(self.proto.try_recv(&self.inner).await?)
    }

    async fn peek_packet_id(&mut self) -> Result<PacketId> {
        Ok(self.proto.peek_packet_id(&self.inner).await?)
    }
}

// ========================================== impl From ========================================= \\

impl From<channel::SendError<packets::Hello>> for Error {
    #[inline]
    fn from(_: channel::SendError<packets::Hello>) -> Self {
        Error::Hello
    }
}

impl From<channel::SendError<oneshot::Sender<packets::Hello>>> for Error {
    #[inline]
    fn from(_: channel::SendError<oneshot::Sender<packets::Hello>>) -> Self {
        Error::Hello
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
