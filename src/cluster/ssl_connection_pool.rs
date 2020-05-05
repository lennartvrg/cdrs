use openssl::ssl::SslConnector;
use r2d2::{Builder, ManageConnection, Pool};
use std::cell::RefCell;
use std::error::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use std::net::{SocketAddr, ToSocketAddrs};

use crate::authenticators::Authenticator;
use crate::cluster::ConnectionPool;
use crate::cluster::{startup, NodeSslConfig};
use crate::compression::Compression;
use crate::error;
use crate::frame::parser::parse_frame;
use crate::frame::{Frame, IntoBytes};
use crate::transport::TransportTls;

/// Shortcut for `bb8::Pool` type of SSL-based CDRS connections.
pub type SslConnectionPool<A> = ConnectionPool<SslConnectionsManager<A>>;

/// `bb8::Pool` of SSL-based CDRS connections.
///
/// Used internally for SSL Session for holding connections to a specific Cassandra node.
pub async fn new_ssl_pool<'a, A: Authenticator + Send + Sync + 'static>(
    node_config: NodeSslConfig<'a, A>,
) -> error::Result<SslConnectionPool<A>> {
    let manager = SslConnectionsManager::new(
        node_config.addr,
        node_config.authenticator,
    );

    let pool = Builder::new()
        .max_size(node_config.max_size)
        .min_idle(node_config.min_idle)
        .max_lifetime(node_config.max_lifetime)
        .idle_timeout(node_config.idle_timeout)
        .connection_timeout(node_config.connection_timeout)
        .build(manager)
        .await
        .map_err(|err| error::Error::from(err.to_string()))?;

    let addr = node_config
        .addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| error::Error::from("Cannot parse address"))?;

    Ok(SslConnectionPool::new(pool, addr))
}

/// `bb8` connection manager.
#[derive(Debug)]
pub struct SslConnectionsManager<A> {
    addr: String,
    auth: A,
}

impl<A> SslConnectionsManager<A> {
    pub fn new<S: ToString>(addr: S, auth: A) -> Self {
        SslConnectionsManager {
            addr: addr.to_string(),
            auth,
        }
    }
}

#[async_trait]
impl<A: Authenticator + 'static + Send + Sync> ManageConnection for SslConnectionsManager<A> {
    type Connection = Mutex<TransportTls>;
    type Error = error::Error;

    async fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let transport = Mutex::new(TransportTls::new(&self.addr).await?);
        startup(&transport, &self.auth).await?;

        Ok(transport)
    }

    async fn is_valid(&self, conn: Self::Connection) -> Result<Self::Connection, Self::Error> {
        let options_frame = Frame::new_req_options().into_cbytes();
        conn.lock().await.write(options_frame.as_slice()).await?;

        parse_frame(&conn, &Compression::None {}).await.map(|_| conn)
    }

    fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
        false
    }
}
