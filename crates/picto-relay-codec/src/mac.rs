use std::fmt;

/// Octets in an IEEE 802 MAC address.
pub const MAC_ADDRESS_LEN: usize = 6;

/// A 48-bit IEEE 802 MAC address.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacAddress(pub [u8; MAC_ADDRESS_LEN]);

impl MacAddress {
    /// Returns the raw six address octets.
    #[must_use]
    pub const fn octets(self) -> [u8; MAC_ADDRESS_LEN] {
        self.0
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, octet) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(":")?;
            }
            write!(formatter, "{octet:02x}")?;
        }
        Ok(())
    }
}
