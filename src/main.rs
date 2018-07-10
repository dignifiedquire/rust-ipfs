extern crate bytes;
extern crate futures;
extern crate libp2p_core as swarm;
extern crate libp2p_identify;
extern crate libp2p_mplex;
extern crate libp2p_peerstore;
extern crate libp2p_ping;
extern crate libp2p_tcp_transport;
extern crate rand;
extern crate tokio_core;
extern crate tokio_io;

use bytes::Bytes;
use futures::Future;
use libp2p_identify::{IdentifyInfo, IdentifyOutput, IdentifyProtocolConfig, IdentifyTransport};
use libp2p_mplex::BufferedMultiplexConfig;
use libp2p_peerstore::{PeerAccess, PeerId, Peerstore};
use libp2p_ping::Ping;
use std::io;
use std::ops::Deref;
use std::sync::{Arc, RwLock};
use swarm::{ConnectionUpgrade, Endpoint, Multiaddr, PublicKey, Transport};
use tokio_io::{AsyncRead, AsyncWrite};

fn main() {
    let addr = "/ip4/127.0.0.1/tcp/9999";
    let to_listen: Option<Multiaddr> = Some(addr.parse().unwrap());

    let mut core = tokio_core::reactor::Core::new().unwrap();

    let peer_store = Arc::new(libp2p_peerstore::memory_peerstore::MemoryPeerstore::empty());
    let my_key = {
        let key = (0..2048).map(|_| rand::random::<u8>()).collect();
        PublicKey::Rsa(key)
    };
    let my_peer_id = PeerId::from_public_key(my_key.clone());

    let listened_addrs = Arc::new(RwLock::new(vec![]));

    let transport = libp2p_tcp_transport::TcpConfig::new(core.handle())
        .with_upgrade(swarm::upgrade::PlainTextConfig)
        .with_upgrade(BufferedMultiplexConfig::<[_; 256]>::new())
        .into_connection_reuse();

    let transport = {
        let listened_addrs = listened_addrs.clone();
        let to_listen = to_listen.clone();
        IdentifyTransport::new(transport.clone(), peer_store.clone()).map(move |out, _| {
            if let (Some(ref observed), Some(ref to_listen)) = (out.observed_addr, to_listen) {
                if let Some(viewed_from_outside) = transport.nat_traversal(to_listen, observed) {
                    listened_addrs.write().unwrap().push(viewed_from_outside);
                }
            }
            out.socket
        })
    };

    let upgrade = ConnectionUpgrader {
        identify: libp2p_identify::IdentifyProtocolConfig,
    };

    let my_peer_id_clone = my_peer_id.clone();
    let listened_addrs_clone = listened_addrs.clone();
    let (swarm_controller, swarm_future) = swarm::swarm(
        transport.with_upgrade(upgrade),
        move |upgrade, client_addr| match upgrade {
            FinalUpgrade::Identify(IdentifyOutput::Sender { sender, .. }) => sender.send(
                IdentifyInfo {
                    public_key: my_key.clone(),
                    protocol_version: "ipfs/1.0.0".to_owned(),
                    agent_version: "rust-libp2p/1.0.0".to_owned(),
                    listen_addrs: listened_addrs.read().unwrap().to_vec(),
                    protocols: vec!["/ipfs/id/1.0.0".to_owned()],
                },
                &client_addr.wait().unwrap(),
            ),
            FinalUpgrade::Identify(IdentifyOutput::RemoteInfo { .. }) => {
                unreachable!("We are never dialing with the identify protocol")
            }
        },
    );

    // Listening on the address passed as program argument, or a default one.
    if let Some(to_listen) = to_listen {
        let actual_addr = swarm_controller
            .listen_on(to_listen)
            .expect("failed to listen to multiaddress");
        println!("Now listening on {}", actual_addr);
        listened_addrs_clone.write().unwrap().push(actual_addr);
    }

    // Runs until everything is finished.
    core.run(swarm_future).unwrap();
}

// todo move into another file
pub enum FinalUpgrade<C> {
    Identify(IdentifyOutput<C>),
}

#[derive(Clone)]
pub struct ConnectionUpgrader {
    identify: IdentifyProtocolConfig,
}

impl<C, Maf> ConnectionUpgrade<C, Maf> for ConnectionUpgrader
where
    C: AsyncRead + AsyncWrite + 'static, // TODO: 'static :-/
    Maf: Future<Item = Multiaddr, Error = io::Error> + 'static,
{
    type NamesIter = ::std::vec::IntoIter<(Bytes, usize)>;
    type UpgradeIdentifier = usize;
    type MultiaddrFuture = <IdentifyProtocolConfig as ConnectionUpgrade<C, Maf>>::MultiaddrFuture;
    type Future = Box<Future<Item = (Self::Output, Self::MultiaddrFuture), Error = Maf::Error>>;
    type Output = FinalUpgrade<C>;

    #[inline]
    fn protocol_names(&self) -> Self::NamesIter {
        vec![(Bytes::from("/ipfs/id/1.0.0"), 0)].into_iter()
    }

    fn upgrade(
        self,
        socket: C,
        id: Self::UpgradeIdentifier,
        ty: Endpoint,
        remote_addr: Maf,
    ) -> Self::Future {
        match id {
            0 => Box::new(
                self.identify
                    .upgrade(socket, (), ty, remote_addr)
                    .map(|upg| (FinalUpgrade::Identify(upg.0), upg.1)),
            ),
            _ => unreachable!(),
        }
    }
}

impl<C> From<IdentifyOutput<C>> for FinalUpgrade<C> {
    #[inline]
    fn from(upgr: IdentifyOutput<C>) -> Self {
        FinalUpgrade::Identify(upgr)
    }
}
