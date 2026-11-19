//! The machine's network fingerprint: the set of addresses its interfaces
//! hold. The gate reads it while closed to notice that the network changed,
//! or that there is none.

use std::collections::BTreeSet;
use std::io;
use std::net::IpAddr;

/// Reads the set of interface addresses that could carry a request off this
/// machine. An empty set means there is no network.
pub(crate) trait NetworkFingerprint: Send + Sync {
    fn read(&self) -> io::Result<BTreeSet<IpAddr>>;
}

/// The real fingerprint, read from the operating system's interface list.
pub(crate) struct InterfaceAddresses;

impl NetworkFingerprint for InterfaceAddresses {
    fn read(&self) -> io::Result<BTreeSet<IpAddr>> {
        Ok(if_addrs::get_if_addrs()?
            .into_iter()
            .filter(counts_as_network)
            .map(|interface| interface.ip())
            .collect())
    }
}

/// Loopback addresses never leave the machine. Link-local ones are left out
/// too: macOS keeps an `fe80::` address on its `utun` and `awdl` interfaces
/// with Wi-Fi off and no cable, so counting them would make the set never
/// empty and "no network" undetectable.
fn counts_as_network(interface: &if_addrs::Interface) -> bool {
    !interface.is_loopback() && !interface.is_link_local()
}
