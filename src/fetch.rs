//! Busca de uma imagem por endereço (GIF colado no chat).
//!
//! Ao copiar um GIF, o navegador só põe na área de transferência um PNG do primeiro
//! quadro e o endereço do original. O Concord busca o original pelo endereço, com
//! proteção contra SSRF (usar o servidor para alcançar a rede interna):
//! - só `https://` na porta 443, também nos redirecionamentos (no máximo 3);
//! - o DNS é resolvido aqui e **só IPs públicos** são usados na conexão (endereços
//!   privados, da própria máquina, de metadados da nuvem 169.254.169.254 etc. são
//!   recusados), o que também vale contra troca de DNS no meio do caminho;
//! - até 25 MB e 15 s; sem cookies; o resultado só é aceito se for imagem de verdade
//!   (e passa pela mesma limpeza de metadados de um envio comum).

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use crate::media::MAX_FILE_BYTES;

/// Só endereços roteáveis na internet pública.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let s = v6.segments();
            // NAT64 (64:ff9b::/96) aponta para um IPv4: vale a regra do IPv4.
            if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
                let o = v6.octets();
                return is_public_v4(Ipv4Addr::new(o[12], o[13], o[14], o[15]));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
                || (s[0] & 0xffc0) == 0xfe80 // link-local
                || (s[0] & 0xffc0) == 0xfec0 // site-local (antigo)
                || s[0] == 0x2001 && s[1] == 0x0db8 // documentação
                || v6 == Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local() // 169.254/16, inclui metadados da AWS
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // CGNAT 100.64/10
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0/24
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // testes 198.18/15
        || o[0] >= 240)
}

struct PublicOnly;

impl ureq::Resolver for PublicOnly {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        let addrs: Vec<SocketAddr> = netloc
            .to_socket_addrs()?
            .filter(|a| a.port() == 443 && is_public(a.ip()))
            .collect();
        if addrs.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "endereço não público",
            ));
        }
        Ok(addrs)
    }
}

/// Baixa o conteúdo (bloqueante: chame dentro de `spawn_blocking`).
pub fn fetch(url: &str) -> Result<Vec<u8>, &'static str> {
    const BAD: &str = "Não foi possível buscar a imagem original";
    if !url.starts_with("https://") || url.len() > 2048 {
        return Err(BAD);
    }
    let agent = ureq::AgentBuilder::new()
        .resolver(PublicOnly)
        .https_only(true)
        .redirects(3)
        .timeout(Duration::from_secs(15))
        .user_agent("Concord/1.0 (busca de GIF colado no chat)")
        .build();
    let resp = agent.get(url).call().map_err(|_| BAD)?;
    let mut data = Vec::new();
    resp.into_reader()
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|_| BAD)?;
    if data.len() as u64 > MAX_FILE_BYTES {
        return Err("A imagem original passa de 25 MB");
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn so_ips_publicos() {
        for bad in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a9fe:a9fe",
        ] {
            assert!(
                !is_public(bad.parse().unwrap()),
                "{bad} deveria ser bloqueado"
            );
        }
        for good in ["8.8.8.8", "151.101.1.1", "2606:4700::1111"] {
            assert!(is_public(good.parse().unwrap()), "{good} deveria passar");
        }
        assert!(fetch("http://exemplo.com/a.gif").is_err());
        assert!(fetch("file:///etc/passwd").is_err());
    }
}
