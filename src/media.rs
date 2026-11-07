//! Arquivos enviados no chat: identifica o formato pelo conteúdo (não pela
//! extensão). Imagens: lê as dimensões e **remove metadados** (EXIF com GPS, XMP,
//! IPTC, comentários). Vídeos MP4/MOV: neutraliza os blocos de metadados (onde o
//! celular grava a localização) e zera as datas de gravação. LGPD: fotos e vídeos
//! de celular costumam trazer a localização exata. O navegador já recodifica as
//! imagens antes do envio; esta limpeza vale também contra clientes modificados.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl Kind {
    pub fn mime(self) -> &'static str {
        match self {
            Kind::Png => "image/png",
            Kind::Jpeg => "image/jpeg",
            Kind::Gif => "image/gif",
            Kind::Webp => "image/webp",
        }
    }
}

pub fn detect(b: &[u8]) -> Option<Kind> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Kind::Png)
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Kind::Jpeg)
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some(Kind::Gif)
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some(Kind::Webp)
    } else {
        None
    }
}

/// Arquivo pronto para guardar. `kind` só existe para imagens.
pub struct Clean {
    pub kind: Kind,
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub other_mime: Option<&'static str>,
}

impl Clean {
    pub fn file(data: Vec<u8>, mime: &'static str) -> Self {
        Clean {
            kind: Kind::Png,
            data,
            width: 0,
            height: 0,
            other_mime: Some(mime),
        }
    }
    pub fn mime(&self) -> &'static str {
        self.other_mime.unwrap_or_else(|| self.kind.mime())
    }
    pub fn image_kind(&self) -> Option<Kind> {
        self.other_mime.is_none().then_some(self.kind)
    }
}

/// Identifica, remove metadados e lê as dimensões. Recusa arquivos malformados.
pub fn sanitize(input: &[u8]) -> Result<Clean, &'static str> {
    let kind = detect(input).ok_or("Formato não suportado (use PNG, JPEG, GIF ou WebP)")?;
    let data = match kind {
        Kind::Png => strip_png(input),
        Kind::Jpeg => strip_jpeg(input),
        Kind::Gif => strip_gif(input),
        Kind::Webp => strip_webp(input),
    }
    .ok_or("Imagem corrompida ou inválida")?;
    let (width, height) =
        dimensions(kind, &data).ok_or("Não foi possível ler as dimensões da imagem")?;
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return Err("Dimensões de imagem inválidas");
    }
    Ok(Clean {
        kind,
        data,
        width,
        height,
        other_mime: None,
    })
}

fn be16(b: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize)
}
fn be32(b: &[u8], i: usize) -> Option<usize> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize)
}
fn le16(b: &[u8], i: usize) -> Option<u32> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32)
}
fn le24(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(i)?,
        *b.get(i + 1)?,
        *b.get(i + 2)?,
        0,
    ]))
}
fn le32(b: &[u8], i: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize)
}

// ---------- PNG: blocos de texto, EXIF e data ----------
fn strip_png(b: &[u8]) -> Option<Vec<u8>> {
    let mut out = b.get(..8)?.to_vec();
    let mut i = 8;
    loop {
        let len = be32(b, i)?;
        let end = i.checked_add(12)?.checked_add(len)?;
        let chunk = b.get(i..end)?;
        let kind = &chunk[4..8];
        if !matches!(kind, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME") {
            out.extend_from_slice(chunk);
        }
        i = end;
        if kind == b"IEND" {
            return Some(out);
        }
    }
}

// ---------- JPEG: segmentos APPn (EXIF/XMP/IPTC) e comentários ----------
fn strip_jpeg(b: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![0xFF, 0xD8];
    let mut i = 2;
    loop {
        if *b.get(i)? != 0xFF {
            return None;
        }
        let mut m = *b.get(i + 1)?;
        while m == 0xFF {
            i += 1;
            m = *b.get(i + 1)?;
        }
        // Início dos dados da imagem (ou fim): copia o resto como está.
        if m == 0xDA || m == 0xD9 {
            out.extend_from_slice(b.get(i..)?);
            return Some(out);
        }
        if (0xD0..=0xD7).contains(&m) || m == 0x01 {
            out.extend_from_slice(&[0xFF, m]);
            i += 2;
            continue;
        }
        let len = be16(b, i + 2)?;
        if len < 2 {
            return None;
        }
        let end = i + 2 + len;
        let seg = b.get(i..end)?;
        // Mantém APP0 (JFIF), APP2 (perfil de cor ICC) e APP14 (Adobe, afeta cores).
        let drop = m == 0xFE || ((0xE1..=0xEF).contains(&m) && m != 0xE2 && m != 0xEE);
        if !drop {
            out.extend_from_slice(seg);
        }
        i = end;
    }
}

// ---------- GIF: comentários e extensões de aplicação (XMP) ----------
fn strip_gif(b: &[u8]) -> Option<Vec<u8>> {
    let flags = *b.get(10)?;
    let mut i = 13;
    if flags & 0x80 != 0 {
        i += 3 * (1usize << ((flags & 0x07) + 1));
    }
    let mut out = b.get(..i)?.to_vec();
    let sub_blocks_end = |mut j: usize| -> Option<usize> {
        loop {
            let n = *b.get(j)? as usize;
            j += 1 + n;
            if n == 0 {
                return Some(j);
            }
        }
    };
    let mut frames = 0usize;
    loop {
        let Some(&byte) = b.get(i) else {
            // Terminou sem o byte final: aceita se já houver ao menos um quadro.
            out.push(0x3B);
            return (frames > 0).then_some(out);
        };
        match byte {
            0x3B => {
                out.push(0x3B);
                return Some(out);
            }
            0x21 => {
                let label = *b.get(i + 1)?;
                let end = sub_blocks_end(i + 2)?;
                let keep = match label {
                    0xFE => false, // comentário
                    0xFF => {
                        // Mantém só o laço de animação; descarta XMP e outros.
                        let id = b.get(i + 3..i + 14)?;
                        id == b"NETSCAPE2.0" || id == b"ANIMEXTS1.0"
                    }
                    _ => true,
                };
                if keep {
                    out.extend_from_slice(b.get(i..end)?);
                }
                i = end;
            }
            0x2C => {
                let f = *b.get(i + 9)?;
                let mut j = i + 10;
                if f & 0x80 != 0 {
                    j += 3 * (1usize << ((f & 0x07) + 1));
                }
                j += 1; // tamanho mínimo do código LZW
                let Some(end) = sub_blocks_end(j).filter(|&e| e <= b.len()) else {
                    // Último quadro cortado: fica com os quadros completos.
                    out.push(0x3B);
                    return (frames > 0).then_some(out);
                };
                out.extend_from_slice(&b[i..end]);
                frames += 1;
                i = end;
            }
            // Lixo depois dos quadros (preenchimento): encerra ali.
            _ if frames > 0 => {
                out.push(0x3B);
                return Some(out);
            }
            _ => return None,
        }
    }
}

// ---------- WebP: blocos EXIF e XMP ----------
fn strip_webp(b: &[u8]) -> Option<Vec<u8>> {
    let riff_end = 8 + le32(b, 4)?;
    let body = b.get(12..riff_end.min(b.len()))?;
    let mut chunks = Vec::new();
    let mut i = 0;
    while i + 8 <= body.len() {
        let size = le32(body, i + 4)?;
        let padded = size + (size & 1);
        let end = (i + 8 + padded).min(body.len());
        let fourcc = &body[i..i + 4];
        if fourcc != b"EXIF" && fourcc != b"XMP " {
            let mut chunk = body.get(i..end)?.to_vec();
            if fourcc == b"VP8X" && chunk.len() > 8 {
                chunk[8] &= !(0x08 | 0x04); // desliga os sinalizadores de EXIF e XMP
            }
            chunks.push(chunk);
        }
        i = end;
    }
    let total: usize = chunks.iter().map(Vec::len).sum::<usize>() + 4;
    let mut out = Vec::with_capacity(total + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    for c in chunks {
        out.extend_from_slice(&c);
    }
    Some(out)
}

pub fn dimensions(kind: Kind, b: &[u8]) -> Option<(u32, u32)> {
    match kind {
        Kind::Png => Some((be32(b, 16)? as u32, be32(b, 20)? as u32)),
        Kind::Gif => Some((le16(b, 6)?, le16(b, 8)?)),
        Kind::Jpeg => {
            let mut i = 2;
            loop {
                if *b.get(i)? != 0xFF {
                    return None;
                }
                let m = *b.get(i + 1)?;
                if matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
                    return Some((be16(b, i + 7)? as u32, be16(b, i + 5)? as u32));
                }
                if m == 0xDA || m == 0xD9 {
                    return None;
                }
                i += 2 + be16(b, i + 2)?;
            }
        }
        Kind::Webp => {
            let c = b.get(12..)?;
            match c.get(0..4)? {
                b"VP8X" => Some((le24(c, 12)? + 1, le24(c, 15)? + 1)),
                b"VP8 " => Some((le16(c, 14)? & 0x3FFF, le16(c, 16)? & 0x3FFF)),
                b"VP8L" => {
                    let bits = u32::from_le_bytes(c.get(9..13)?.try_into().ok()?);
                    Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
                }
                _ => None,
            }
        }
    }
}

// ---------- outros arquivos (vídeo, áudio, documentos...) ----------

/// Tamanho máximo de um arquivo do chat.
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;

/// Resultado do preparo de um arquivo enviado.
pub enum Prepared {
    Image(Clean),
    /// Qualquer outro arquivo. `mime` vem do conteúdo, nunca do que o navegador disse.
    File {
        data: Vec<u8>,
        mime: &'static str,
    },
}

/// Identifica o arquivo pelo conteúdo e limpa o que for possível.
pub fn prepare(input: &[u8]) -> Result<Prepared, &'static str> {
    if detect(input).is_some() {
        return sanitize(input).map(Prepared::Image);
    }
    let b = input;
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        let brand = &b[8..12];
        // Fotos HEIC/AVIF guardam EXIF com GPS e o navegador nem sempre as converte:
        // em vez de repassar a localização, pede outro formato.
        if matches!(
            brand,
            b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1" | b"avif" | b"avis"
        ) {
            return Err("Foto em HEIC/AVIF: envie em JPEG ou PNG (no iPhone: Ajustes › Câmera › Formatos › Mais Compatível)");
        }
        let mut data = input.to_vec();
        strip_isobmff(&mut data);
        let mime = if matches!(brand, b"M4A " | b"M4B " | b"M4P ") {
            "audio/mp4"
        } else {
            "video/mp4"
        };
        return Ok(Prepared::File { data, mime });
    }
    let mime = if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        "video/webm"
    } else if b.starts_with(b"OggS") {
        "audio/ogg"
    } else if b.starts_with(b"ID3") || (b.len() > 1 && b[0] == 0xFF && b[1] & 0xE0 == 0xE0) {
        "audio/mpeg"
    } else if b.starts_with(b"fLaC") {
        "audio/flac"
    } else if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        "audio/wav"
    } else if b.starts_with(b"%PDF-") {
        "application/pdf"
    } else {
        "application/octet-stream"
    };
    Ok(Prepared::File {
        data: input.to_vec(),
        mime,
    })
}

/// MP4/MOV: troca os blocos de metadados por `free` zerados (mesmo tamanho, então nenhum
/// deslocamento muda e o vídeo continua tocando) e zera as datas de criação/modificação.
/// É onde ficam a localização (©xyz, com.apple.quicktime.location), o modelo do aparelho e o
/// software. O conteúdo de áudio e vídeo não é tocado.
pub fn strip_isobmff(buf: &mut [u8]) {
    walk_boxes(buf, 0, buf.len(), 0);
}

fn walk_boxes(b: &mut [u8], start: usize, end: usize, depth: u8) {
    let mut i = start;
    while i + 8 <= end && depth < 10 {
        let mut size = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let mut header = 8;
        if size == 1 {
            if i + 16 > end {
                return;
            }
            size = u64::from_be_bytes(b[i + 8..i + 16].try_into().unwrap()) as usize;
            header = 16;
        } else if size == 0 {
            size = end - i;
        }
        if size < header || size > end - i {
            return;
        }
        let kind: [u8; 4] = b[i + 4..i + 8].try_into().unwrap();
        match &kind {
            b"udta" | b"meta" | b"uuid" | b"XMP_" => {
                // Vira um bloco vazio do mesmo tamanho: o tipo muda e o conteúdo é zerado.
                b[i + 4..i + 8].copy_from_slice(b"free");
                b[i + header..i + size].fill(0);
            }
            b"moov" | b"trak" | b"mdia" | b"minf" | b"edts" => {
                walk_boxes(b, i + header, i + size, depth + 1)
            }
            b"mvhd" | b"tkhd" | b"mdhd" => zero_box_times(b, i + header, i + size),
            _ => {}
        }
        i += size;
    }
}

/// Caixas "full box": versão 0 tem datas de 32 bits; versão 1, de 64 bits.
fn zero_box_times(b: &mut [u8], start: usize, end: usize) {
    let Some(&version) = b.get(start) else { return };
    let len = if version == 1 { 16 } else { 8 };
    if start + 4 + len <= end {
        b[start + 4..start + 4 + len].fill(0);
    }
}

// ---------- áudio do Soundpad ----------

/// Duração máxima de um som (com uma pequena folga para arredondamentos).
pub const MAX_SOUND_MS: u32 = 5_300;

/// Som limpo: WAV PCM 16 bits reconstruído só com os blocos "fmt " e "data".
pub struct CleanSound {
    pub data: Vec<u8>,
    pub duration_ms: u32,
}

/// Aceita só WAV PCM 16 bits (o navegador converte qualquer formato para isso antes
/// de enviar) e reconstrói o arquivo do zero: blocos LIST/INFO, id3, bext etc.
/// (onde ficam metadados como autor, software e datas) são descartados.
pub fn sanitize_wav(input: &[u8]) -> Result<CleanSound, &'static str> {
    const BAD: &str = "Áudio inválido (use um arquivo de som comum, até 5 segundos)";
    if input.len() < 12 || &input[0..4] != b"RIFF" || &input[8..12] != b"WAVE" {
        return Err(BAD);
    }
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // formato, canais, taxa, bits
    let mut data: Option<&[u8]> = None;
    let mut i = 12usize;
    while i + 8 <= input.len() {
        let id = &input[i..i + 4];
        let len = u32::from_le_bytes(input[i + 4..i + 8].try_into().unwrap()) as usize;
        let body_start = i + 8;
        let body_end = body_start.checked_add(len).ok_or(BAD)?.min(input.len());
        let body = &input[body_start..body_end];
        match id {
            b"fmt " if body.len() >= 16 => {
                fmt = Some((
                    u16::from_le_bytes([body[0], body[1]]),
                    u16::from_le_bytes([body[2], body[3]]),
                    u32::from_le_bytes([body[4], body[5], body[6], body[7]]),
                    u16::from_le_bytes([body[14], body[15]]),
                ));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        // Blocos têm tamanho par (byte de preenchimento).
        i = body_start.saturating_add(len).saturating_add(len & 1);
    }
    let (format, channels, rate, bits) = fmt.ok_or(BAD)?;
    let pcm = data.ok_or(BAD)?;
    if format != 1
        || bits != 16
        || !(1..=2).contains(&channels)
        || !(8_000..=48_000).contains(&rate)
    {
        return Err(BAD);
    }
    let frame = 2 * channels as usize;
    let pcm = &pcm[..pcm.len() - pcm.len() % frame];
    if pcm.is_empty() {
        return Err(BAD);
    }
    let duration_ms = ((pcm.len() / frame) as u64 * 1000 / rate as u64) as u32;
    if duration_ms > MAX_SOUND_MS {
        return Err("O som pode ter no máximo 5 segundos");
    }
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * frame as u32).to_le_bytes());
    out.extend_from_slice(&(frame as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    Ok(CleanSound {
        data: out,
        duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0, 0, 0, 0]); // CRC (não verificado aqui)
        c
    }

    #[test]
    fn png_perde_texto_e_exif_e_mantem_imagem() {
        let mut ihdr = 640u32.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&480u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(png_chunk(b"IHDR", &ihdr));
        png.extend(png_chunk(b"eXIf", b"GPS -23.6,-46.6"));
        png.extend(png_chunk(b"tEXt", b"Author\0Fulano"));
        png.extend(png_chunk(b"IDAT", &[1, 2, 3]));
        png.extend(png_chunk(b"IEND", &[]));
        let c = sanitize(&png).unwrap();
        assert_eq!((c.kind, c.width, c.height), (Kind::Png, 640, 480));
        let s = String::from_utf8_lossy(&c.data);
        assert!(!s.contains("GPS") && !s.contains("Fulano") && s.contains("IDAT"));
    }

    #[test]
    fn jpeg_perde_exif_e_mantem_jfif_e_dimensoes() {
        let mut j = vec![0xFF, 0xD8];
        j.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x07]);
        j.extend_from_slice(b"JFIF\0");
        let exif = b"Exif\0\0GPS-23.6";
        j.extend_from_slice(&[0xFF, 0xE1, 0x00, (exif.len() + 2) as u8]);
        j.extend_from_slice(exif);
        j.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x07]);
        j.extend_from_slice(b"oi :)");
        // SOF0: comprimento 11, precisão 8, altura 300, largura 400, 1 componente
        j.extend_from_slice(&[
            0xFF, 0xC0, 0x00, 0x0B, 8, 0x01, 0x2C, 0x01, 0x90, 1, 1, 0x11, 0,
        ]);
        j.extend_from_slice(&[
            0xFF, 0xDA, 0x00, 0x08, 1, 1, 0, 0, 0x3F, 0, 0xAA, 0xBB, 0xFF, 0xD9,
        ]);
        let c = sanitize(&j).unwrap();
        assert_eq!((c.width, c.height), (400, 300));
        let s = String::from_utf8_lossy(&c.data);
        assert!(s.contains("JFIF") && !s.contains("GPS") && !s.contains("oi :)"));
        assert!(
            c.data.ends_with(&[0xAA, 0xBB, 0xFF, 0xD9]),
            "dados da imagem preservados"
        );
    }

    #[test]
    fn gif_perde_comentario_e_xmp_e_mantem_animacao() {
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&[10, 0, 20, 0, 0, 0, 0]); // 10x20, sem paleta global
        g.extend_from_slice(&[0x21, 0xFF, 11]);
        g.extend_from_slice(b"NETSCAPE2.0");
        g.extend_from_slice(&[3, 1, 0, 0, 0]);
        g.extend_from_slice(&[0x21, 0xFF, 11]);
        g.extend_from_slice(b"XMP DataXMP");
        g.extend_from_slice(&[4, b'g', b'p', b's', b'!', 0]);
        g.extend_from_slice(&[0x21, 0xFE, 5]);
        g.extend_from_slice(b"segredo");
        g.truncate(g.len() - 2); // comentário de 5 bytes
        g.push(0);
        g.extend_from_slice(&[0x2C, 0, 0, 0, 0, 10, 0, 20, 0, 0, 2, 2, 0x4C, 0x01, 0]);
        g.push(0x3B);
        let c = sanitize(&g).unwrap();
        assert_eq!((c.width, c.height), (10, 20));
        let s = String::from_utf8_lossy(&c.data);
        assert!(s.contains("NETSCAPE2.0") && !s.contains("XMP") && !s.contains("segre"));
        assert_eq!(*c.data.last().unwrap(), 0x3B);
    }

    #[test]
    fn webp_perde_exif_e_ajusta_tamanho_e_sinalizadores() {
        let mut vp8x = vec![0x08 | 0x04, 0, 0, 0];
        vp8x.extend_from_slice(&[99, 0, 0]); // largura - 1 = 99
        vp8x.extend_from_slice(&[49, 0, 0]); // altura - 1 = 49
        let mut body = b"WEBP".to_vec();
        for (cc, data) in [
            (&b"VP8X"[..], vp8x),
            (&b"EXIF"[..], b"GPS-23".to_vec()),
            (&b"VP8L"[..], vec![0x2F, 0, 0, 0, 0]),
        ] {
            body.extend_from_slice(cc);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(&data);
            if data.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut w = b"RIFF".to_vec();
        w.extend_from_slice(&(body.len() as u32).to_le_bytes());
        w.extend_from_slice(&body);
        let c = sanitize(&w).unwrap();
        assert_eq!((c.width, c.height), (100, 50));
        assert!(!String::from_utf8_lossy(&c.data).contains("GPS"));
        assert_eq!(
            le32(&c.data, 4).unwrap(),
            c.data.len() - 8,
            "tamanho RIFF corrigido"
        );
        assert_eq!(c.data[20] & 0x0C, 0, "sinalizadores de EXIF/XMP desligados");
    }

    #[test]
    fn recusa_formatos_e_arquivos_invalidos() {
        assert!(sanitize(b"<svg onload=alert(1)>").is_err());
        assert!(sanitize(b"\x89PNG\r\n\x1a\n\x00\x00").is_err());
        assert!(sanitize(&[0xFF, 0xD8, 0xFF, 0xE0]).is_err());
    }

    fn wav(extra: &[u8], samples: usize, rate: u32) -> Vec<u8> {
        let mut v = b"RIFF\0\0\0\0WAVE".to_vec();
        v.extend_from_slice(extra);
        v.extend_from_slice(b"fmt \0\0\0\0\0");
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * 2).to_le_bytes());
        v.extend_from_slice(b"\0\0data");
        v.extend_from_slice(&((samples * 2) as u32).to_le_bytes());
        v.extend(std::iter::repeat_n(7u8, samples * 2));
        v
    }

    #[test]
    fn gif_sem_byte_final_e_aceito() {
        let mut gif = b"GIF89a\x01\0\x01\0\x80\0\0\xff\xff\xff\0\0\0".to_vec();
        gif.extend_from_slice(b",\0\0\0\0\x01\0\x01\0\0\x02\x02D\x01\0");
        let clean = sanitize(&gif).expect("GIF sem 0x3B");
        assert_eq!(*clean.data.last().unwrap(), 0x3B);
        let mut padded = gif.clone();
        padded.extend_from_slice(&[0x3B, 0, 0, 0]);
        assert!(sanitize(&padded).is_ok());
        let mut junk = gif.clone();
        junk.extend_from_slice(&[0, 0, 0]);
        assert!(sanitize(&junk).is_ok());
    }

    #[test]
    fn mp4_perde_localizacao_e_datas() {
        let bx = |kind: &[u8], body: &[u8]| {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(body);
            v
        };
        let mut mvhd = vec![0u8; 100];
        mvhd[4..12].copy_from_slice(&[0xAA; 8]);
        let udta = bx(b"udta", &bx(b"\xa9xyz", b"+23.5-046.6/"));
        let mut moov_body = bx(b"mvhd", &mvhd);
        moov_body.extend(udta);
        let mut file = bx(b"ftyp", b"isom\0\0\0\0isommp41");
        file.extend(bx(b"moov", &moov_body));
        file.extend(bx(b"mdat", b"dados"));
        let Prepared::File { data, mime } = prepare(&file).unwrap() else {
            panic!()
        };
        assert_eq!(mime, "video/mp4");
        assert_eq!(data.len(), file.len());
        assert!(!data.windows(4).any(|w| w == b"udta"));
        assert!(!data.windows(4).any(|w| w == [0xAA; 4]));
        assert!(!data.windows(8).any(|w| w == b"+23.5-04"));
        assert!(data.windows(5).any(|w| w == b"dados"));
        let heic = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
        assert!(prepare(&heic).is_err());
        let Prepared::File { mime, .. } = prepare(b"<html>oi").unwrap() else {
            panic!()
        };
        assert_eq!(mime, "application/octet-stream");
    }

    #[test]
    fn wav_metadata_removed_and_duration_checked() {
        let list = b"LIST\0\0\0INFOIART\0\0\0\0";
        let clean = sanitize_wav(&wav(list, 48_000, 48_000)).unwrap();
        assert_eq!(clean.duration_ms, 1000);
        assert!(!clean.data.windows(4).any(|w| w == b"LIST"));
        assert_eq!(clean.data.len(), 44 + 96_000);
        assert!(sanitize_wav(&wav(b"", 48_000 * 6, 48_000)).is_err());
        assert!(sanitize_wav(b"RIFF....WAVEjunk").is_err());
        assert!(sanitize_wav(b"ID3 nao e wav").is_err());
    }
}
