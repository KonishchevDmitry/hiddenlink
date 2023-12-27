use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy)]
    pub struct ConnectionFlags: u8 {
        const INGRESS = 1 << 0;
        const EGRESS  = 1 << 1;
    }
}