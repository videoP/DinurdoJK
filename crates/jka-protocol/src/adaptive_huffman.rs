//! Adaptive (FGK) Huffman from OpenJK `qcommon/huffman.cpp`.
//!
//! This is NOT the trained codec in `message.rs`. Stock JKA uses it for exactly
//! one wire format: `NET_OutOfBandData` compresses the connectionless `connect`
//! packet with `Huff_Compress(msg, 12)`, and `SV_DirectConnect` undoes it with
//! `Huff_Decompress(msg, 12)`. The server's decompressor evolves its tree in
//! lock-step with our compressor, so this is a line-by-line port of the
//! pointer-based C (nodes and `head` slots become arena indices) rather than an
//! independent FGK implementation that might order sibling swaps differently.

const HMAX: usize = 256;
const NYT: usize = HMAX;
const INTERNAL_NODE: usize = HMAX + 1;
const NIL: usize = usize::MAX;

#[derive(Clone, Copy)]
struct Node {
    left: usize,
    right: usize,
    parent: usize,
    next: usize,
    prev: usize,
    /// Index into `Huff::slots` (C: `node_t **head`).
    head: usize,
    weight: i32,
    symbol: usize,
}

impl Default for Node {
    fn default() -> Self {
        // huff_t is memset to zero in C: NULL links, weight 0, symbol 0.
        Self { left: NIL, right: NIL, parent: NIL, next: NIL, prev: NIL, head: NIL, weight: 0, symbol: 0 }
    }
}

struct Huff {
    tree: usize,
    lhead: usize,
    loc: [usize; HMAX + 1],
    nodes: Vec<Node>,
    /// C `nodePtrs`: each slot holds a node index while in use, or the next
    /// free slot while on the free list (the C code aliases both through
    /// `node_t **`).
    slots: Vec<usize>,
    freelist: usize,
    bloc: usize,
    /// Set when `send` hit maxoffset; the reference output is then invalid.
    overflowed: bool,
}

impl Huff {
    fn new() -> Self {
        let mut huff = Self {
            tree: 0,
            lhead: 0,
            loc: [NIL; HMAX + 1],
            nodes: Vec::with_capacity(768),
            slots: Vec::with_capacity(768),
            freelist: NIL,
            bloc: 0,
            overflowed: false,
        };
        huff.nodes.push(Node { symbol: NYT, weight: 0, ..Node::default() });
        huff.loc[NYT] = 0;
        huff
    }

    fn get_ppnode(&mut self) -> usize {
        if self.freelist == NIL {
            self.slots.push(NIL);
            self.slots.len() - 1
        } else {
            let slot = self.freelist;
            self.freelist = self.slots[slot];
            slot
        }
    }

    fn free_ppnode(&mut self, slot: usize) {
        self.slots[slot] = self.freelist;
        self.freelist = slot;
    }

    fn swap(&mut self, node1: usize, node2: usize) {
        let par1 = self.nodes[node1].parent;
        let par2 = self.nodes[node2].parent;
        if par1 != NIL {
            if self.nodes[par1].left == node1 {
                self.nodes[par1].left = node2;
            } else {
                self.nodes[par1].right = node2;
            }
        } else {
            self.tree = node2;
        }
        if par2 != NIL {
            if self.nodes[par2].left == node2 {
                self.nodes[par2].left = node1;
            } else {
                self.nodes[par2].right = node1;
            }
        } else {
            self.tree = node1;
        }
        self.nodes[node1].parent = par2;
        self.nodes[node2].parent = par1;
    }

    fn swaplist(&mut self, node1: usize, node2: usize) {
        let par1 = self.nodes[node1].next;
        self.nodes[node1].next = self.nodes[node2].next;
        self.nodes[node2].next = par1;

        let par1 = self.nodes[node1].prev;
        self.nodes[node1].prev = self.nodes[node2].prev;
        self.nodes[node2].prev = par1;

        if self.nodes[node1].next == node1 {
            self.nodes[node1].next = node2;
        }
        if self.nodes[node2].next == node2 {
            self.nodes[node2].next = node1;
        }
        for node in [node1, node2] {
            let next = self.nodes[node].next;
            if next != NIL {
                self.nodes[next].prev = node;
            }
        }
        for node in [node1, node2] {
            let prev = self.nodes[node].prev;
            if prev != NIL {
                self.nodes[prev].next = node;
            }
        }
    }

    fn increment(&mut self, node: usize) {
        if node == NIL {
            return;
        }
        let next = self.nodes[node].next;
        if next != NIL && self.nodes[next].weight == self.nodes[node].weight {
            let lnode = self.slots[self.nodes[node].head];
            if lnode != self.nodes[node].parent {
                self.swap(lnode, node);
            }
            self.swaplist(lnode, node);
        }
        let prev = self.nodes[node].prev;
        let head = self.nodes[node].head;
        if prev != NIL && self.nodes[prev].weight == self.nodes[node].weight {
            self.slots[head] = prev;
        } else {
            self.slots[head] = NIL;
            self.free_ppnode(head);
        }
        self.nodes[node].weight += 1;
        let next = self.nodes[node].next;
        if next != NIL && self.nodes[next].weight == self.nodes[node].weight {
            self.nodes[node].head = self.nodes[next].head;
        } else {
            let slot = self.get_ppnode();
            self.nodes[node].head = slot;
            self.slots[slot] = node;
        }
        let parent = self.nodes[node].parent;
        if parent != NIL {
            self.increment(parent);
            // Re-read: the recursive increment may have relinked this node.
            if self.nodes[node].prev == self.nodes[node].parent {
                let parent = self.nodes[node].parent;
                self.swaplist(node, parent);
                let head = self.nodes[node].head;
                if self.slots[head] == node {
                    self.slots[head] = parent;
                }
            }
        }
    }

    fn add_ref(&mut self, ch: u8) {
        let ch = usize::from(ch);
        if self.loc[ch] != NIL {
            self.increment(self.loc[ch]);
            return;
        }
        let lhead = self.lhead;
        let tnode = self.nodes.len();
        self.nodes.push(Node::default());
        let tnode2 = self.nodes.len();
        self.nodes.push(Node::default());

        self.nodes[tnode2].symbol = INTERNAL_NODE;
        self.nodes[tnode2].weight = 1;
        let lnext = self.nodes[lhead].next;
        self.nodes[tnode2].next = lnext;
        if lnext != NIL {
            self.nodes[lnext].prev = tnode2;
            if self.nodes[lnext].weight == 1 {
                self.nodes[tnode2].head = self.nodes[lnext].head;
            } else {
                let slot = self.get_ppnode();
                self.nodes[tnode2].head = slot;
                self.slots[slot] = tnode2;
            }
        } else {
            let slot = self.get_ppnode();
            self.nodes[tnode2].head = slot;
            self.slots[slot] = tnode2;
        }
        self.nodes[lhead].next = tnode2;
        self.nodes[tnode2].prev = lhead;

        self.nodes[tnode].symbol = ch;
        self.nodes[tnode].weight = 1;
        let lnext = self.nodes[lhead].next;
        self.nodes[tnode].next = lnext;
        if lnext != NIL {
            self.nodes[lnext].prev = tnode;
            if self.nodes[lnext].weight == 1 {
                self.nodes[tnode].head = self.nodes[lnext].head;
            } else {
                // "this should never happen" in the reference, including its
                // tnode2 store; preserved verbatim.
                let slot = self.get_ppnode();
                self.nodes[tnode].head = slot;
                self.slots[slot] = tnode2;
            }
        } else {
            let slot = self.get_ppnode();
            self.nodes[tnode].head = slot;
            self.slots[slot] = tnode;
        }
        self.nodes[lhead].next = tnode;
        self.nodes[tnode].prev = lhead;
        self.nodes[tnode].left = NIL;
        self.nodes[tnode].right = NIL;

        let lparent = self.nodes[lhead].parent;
        if lparent != NIL {
            if self.nodes[lparent].left == lhead {
                self.nodes[lparent].left = tnode2;
            } else {
                self.nodes[lparent].right = tnode2;
            }
        } else {
            self.tree = tnode2;
        }
        self.nodes[tnode2].right = tnode;
        self.nodes[tnode2].left = lhead;
        self.nodes[tnode2].parent = lparent;
        self.nodes[lhead].parent = tnode2;
        self.nodes[tnode].parent = tnode2;
        self.loc[ch] = tnode;
        self.increment(self.nodes[tnode2].parent);
    }

    fn add_bit(&mut self, bit: bool, out: &mut Vec<u8>) {
        let byte = self.bloc >> 3;
        if out.len() <= byte {
            out.resize(byte + 1, 0);
        }
        if self.bloc & 7 == 0 {
            out[byte] = 0;
        }
        out[byte] |= u8::from(bit) << (self.bloc & 7);
        self.bloc += 1;
    }

    fn get_bit(&mut self, input: &[u8]) -> u32 {
        let bit = input.get(self.bloc >> 3).map_or(0, |byte| u32::from((byte >> (self.bloc & 7)) & 1));
        self.bloc += 1;
        bit
    }

    fn send(&mut self, node: usize, child: usize, out: &mut Vec<u8>, max_offset: usize) {
        let parent = self.nodes[node].parent;
        if parent != NIL {
            self.send(parent, node, out, max_offset);
        }
        if child != NIL {
            if self.bloc >= max_offset {
                self.bloc = max_offset + 1;
                self.overflowed = true;
                return;
            }
            let right = self.nodes[node].right == child;
            self.add_bit(right, out);
        }
    }

    fn transmit(&mut self, ch: usize, out: &mut Vec<u8>, max_offset: usize) {
        if self.loc[ch] == NIL {
            self.transmit(NYT, out, max_offset);
            for i in (0..8).rev() {
                self.add_bit((ch >> i) & 1 != 0, out);
            }
        } else {
            self.send(self.loc[ch], NIL, out, max_offset);
        }
    }

    fn receive(&mut self, input: &[u8]) -> usize {
        let mut node = self.tree;
        while node != NIL && self.nodes[node].symbol == INTERNAL_NODE {
            node = if self.get_bit(input) != 0 { self.nodes[node].right } else { self.nodes[node].left };
        }
        if node == NIL { 0 } else { self.nodes[node].symbol }
    }
}

/// `Huff_Compress(msg, offset)`: bytes before `offset` are copied verbatim; the
/// rest become a big-endian 16-bit length followed by the adaptive bitstream.
pub fn compress(packet: &[u8], offset: usize) -> Vec<u8> {
    compress_checked(packet, offset).0
}

/// [`compress`] plus whether the bitstream overflowed the reference's
/// `size << 3` budget. Overflowed output is garbage in OpenJK as well (it
/// depends on stale stack bytes and does not decompress), so callers that
/// build wire packets should treat it as an error.
pub fn compress_checked(packet: &[u8], offset: usize) -> (Vec<u8>, bool) {
    if packet.len() <= offset {
        return (packet.to_vec(), false);
    }
    let body = &packet[offset..];
    let size = body.len();
    let mut huff = Huff::new();
    let mut seq = vec![(size >> 8) as u8, (size & 0xff) as u8];
    huff.bloc = 16;
    for &ch in body {
        huff.transmit(usize::from(ch), &mut seq, size << 3);
        huff.add_ref(ch);
    }
    huff.bloc += 8; // "next byte"
    seq.resize(huff.bloc >> 3, 0);
    let mut out = packet[..offset].to_vec();
    out.extend_from_slice(&seq);
    (out, huff.overflowed)
}

/// `Huff_Decompress(msg, offset)`, bounded by `max_size` like the reference.
pub fn decompress(packet: &[u8], offset: usize, max_size: usize) -> Vec<u8> {
    if packet.len() <= offset {
        return packet.to_vec();
    }
    let buffer = &packet[offset..];
    let size = buffer.len();
    let mut count = usize::from(buffer[0]) * 256 + buffer.get(1).copied().map_or(0, usize::from);
    count = count.min(max_size.saturating_sub(offset));
    let mut huff = Huff::new();
    huff.bloc = 16;
    let mut seq = Vec::with_capacity(count);
    for _ in 0..count {
        if (huff.bloc >> 3) > size {
            seq.push(0);
            break;
        }
        let mut ch = huff.receive(buffer);
        if ch == NYT {
            ch = 0;
            for _ in 0..8 {
                ch = (ch << 1) + huff.get_bit(buffer) as usize;
            }
        }
        seq.push(ch as u8);
        huff.add_ref(ch as u8);
    }
    seq.resize(count, 0);
    let mut out = packet[..offset].to_vec();
    out.extend_from_slice(&seq);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_connect_packet_and_keeps_header() {
        // Huff_Compress only round-trips when its output fits the input's bit
        // budget (maxoffset = size << 3); short strings overflow it in the
        // reference too, but real userinfo compresses comfortably.
        let mut packet = [b"\xff\xff\xff\xffconnect \"".as_slice(), REALISTIC_USERINFO, b"\""].concat();
        let compressed = compress(&packet, 12);
        assert_eq!(&compressed[..12], &packet[..12]);
        assert_eq!(decompress(&compressed, 12, 16384), packet);
        // Every byte value and long repetitive runs exercise NYT and swaps.
        packet.extend((0..=255u8).chain(std::iter::repeat(b'a').take(600)).chain((0..=255u8).rev()));
        assert_eq!(decompress(&compress(&packet, 12), 12, 16384), packet);
    }

    const REALISTIC_USERINFO: &[u8] = br"\name\Padawan\rate\25000\snaps\40\model\kyle/default\forcepowers\7-1-032330000000001333\color1\4\color2\4\handicap\100\sex\male\cg_predictItems\1\saber1\single_1\saber2\none\char_color_red\255\char_color_green\255\char_color_blue\255\protocol\26\qport\1234\challenge\-5501";

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn matches_compiled_openjk_huff_compress_vectors() {
        let (mut count, mut overflow_count) = (0, 0);
        for line in include_str!("../tests/fixtures/adaptive-huffman-vectors.tsv")
            .lines()
            .filter(|line| !line.starts_with('#'))
        {
            let fields: Vec<_> = line.split('\t').collect();
            let offset: usize = fields[0].parse().unwrap();
            let input = unhex(fields[1]);
            let expected = unhex(fields[2]);
            let (actual, overflowed) = compress_checked(&input, offset);
            assert_eq!(actual.len(), expected.len(), "length for input {}", fields[1]);
            if overflowed {
                // Past maxoffset the reference skips bytes it never cleared,
                // so its output there is stale stack data; only the length is
                // meaningful, and no valid packet is ever built this way.
                overflow_count += 1;
                continue;
            }
            // Huff_Compress's trailing "next byte" is never written when the
            // bitstream ends on a byte boundary, so the reference emits
            // uninitialized stack from its local seq[] there; this port emits
            // zero. A partially written final byte must still match exactly.
            let last = expected.len() - 1;
            assert_eq!(actual[..last], expected[..last], "input {}", fields[1]);
            assert!(actual[last] == expected[last] || actual[last] == 0, "input {}", fields[1]);
            assert_eq!(decompress(&expected, offset, 1 << 16), input);
            count += 1;
        }
        // 25 inputs fit and are compared byte-for-byte (including real userinfo,
        // every byte value and long random runs); 28 deliberately overflow.
        assert_eq!((count, overflow_count), (25, 28));
    }

    #[test]
    fn first_symbols_are_transmitted_raw_after_nyt() {
        // With an empty tree NYT is the root, so its code is empty and the
        // first symbol is its 8 raw bits, most significant first.
        let compressed = compress(b"0123456789abA", 12);
        assert_eq!(&compressed[12..14], &[0, 1]);
        // 'A' = 0x41 = 01000001 MSB-first into LSB-first bit order.
        assert_eq!(compressed[14], 0b1000_0010);
    }
}
