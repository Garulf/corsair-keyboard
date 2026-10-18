use k100::Rgb;

#[derive(Debug, Clone, PartialEq)]
pub struct ClientLayer {
    pub priority: u8,
    pub colors: Vec<(u8, Rgb, u8)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientLighting {
    Shared(Vec<ClientLayer>),
    Exclusive(ClientLayer),
}

impl ClientLighting {
    pub const NONE: ClientLighting = ClientLighting::Shared(Vec::new());
}

impl Default for ClientLighting {
    fn default() -> Self {
        Self::NONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_client_lighting_is_an_empty_shared_stack() {
        assert_eq!(ClientLighting::default(), ClientLighting::Shared(vec![]));
    }
}
