
// polynomial: x^32 + x^29 + x^28 + x^25 + x^23 + x^22 + x^10 + x^9 + x^7 + x^4 + x^3 + 1
// bits: 100110010110000000000011010011001
// Selected from: https://users.ece.cmu.edu/~koopman/crc/hd6.html

// lsb    msb
//  v      v
//  11111111 111111111111111111111111 00000000
//  -------- ------------------------ --------
//  F   F    F   F   F   F   F   F             -> Initial CRC of !0x00000000, XORed by input byte 0x00
//
//  10011001 011000000000001101001100 1
//   ------- ------------------------ -
//   C   4    3   0   0   6   9   9            -> XOR factor is 0x9960034C
//
//   1100110 100111111111110010110011 1
//   1001100 101100000000000110100110 01
//
//    101010 001011111111110100010101 11
//    100110 010110000000000011010011 001
//
//     01100 011101111111110111000110 111
//     00000 000000000000000000000000 0000
//
//      1100 011101111111110111000110 1110
//      1001 100101100000000000110100 11001
//
//       101 111000011111110111110010 00101
//       100 110010110000000000011010 011001
//
//        01 001010101111110111101000 010011
//        00 000000000000000000000000 0000000
//
//         1 001010101111110111101000 0100110
//         1 001100101100000000000110 10011001
//
//           000110000011110111101110 11010101
//           ------------------------ --------
//           8   1   C   B   7   7    B   A    -> Partial result for 0x00 is !0xAB77BC18 = 0x548843E7

#[cfg(test)]
fn compute_slow(initial_crc: u32, data: &[u8]) -> u32 {
    let mut reg = !initial_crc;
    for &byte in data.iter() {
        reg ^= byte as u32;
        for _ in 0 .. 8 {
            reg = if reg & 0x00000001 != 0 {
                (reg >> 1) ^ 0x9960034C
            } else {
                reg >> 1
            };
        }
    }
    return !reg;
}

static PARTIAL_RESULTS: [u32; 256] = [
    0x548843E7, 0x9AD08156, 0xFAF9C01C, 0x34A102AD, 0x3AAB4288, 0xF4F38039, 0x94DAC173, 0x5A8203C2,
    0x88CE4139, 0x46968388, 0x26BFC2C2, 0xE8E70073, 0xE6ED4056, 0x28B582E7, 0x489CC3AD, 0x86C4011C,
    0xDEC440C2, 0x109C8273, 0x70B5C339, 0xBEED0188, 0xB0E741AD, 0x7EBF831C, 0x1E96C256, 0xD0CE00E7,
    0x0282421C, 0xCCDA80AD, 0xACF3C1E7, 0x62AB0356, 0x6CA14373, 0xA2F981C2, 0xC2D0C088, 0x0C880239,
    0x72D04334, 0xBC888185, 0xDCA1C0CF, 0x12F9027E, 0x1CF3425B, 0xD2AB80EA, 0xB282C1A0, 0x7CDA0311,
    0xAE9641EA, 0x60CE835B, 0x00E7C211, 0xCEBF00A0, 0xC0B54085, 0x0EED8234, 0x6EC4C37E, 0xA09C01CF,
    0xF89C4011, 0x36C482A0, 0x56EDC3EA, 0x98B5015B, 0x96BF417E, 0x58E783CF, 0x38CEC285, 0xF6960034,
    0x24DA42CF, 0xEA82807E, 0x8AABC134, 0x44F30385, 0x4AF943A0, 0x84A18111, 0xE488C05B, 0x2AD002EA,
    0x18384241, 0xD66080F0, 0xB649C1BA, 0x7811030B, 0x761B432E, 0xB843819F, 0xD86AC0D5, 0x16320264,
    0xC47E409F, 0x0A26822E, 0x6A0FC364, 0xA45701D5, 0xAA5D41F0, 0x64058341, 0x042CC20B, 0xCA7400BA,
    0x92744164, 0x5C2C83D5, 0x3C05C29F, 0xF25D002E, 0xFC57400B, 0x320F82BA, 0x5226C3F0, 0x9C7E0141,
    0x4E3243BA, 0x806A810B, 0xE043C041, 0x2E1B02F0, 0x201142D5, 0xEE498064, 0x8E60C12E, 0x4038039F,
    0x3E604292, 0xF0388023, 0x9011C169, 0x5E4903D8, 0x504343FD, 0x9E1B814C, 0xFE32C006, 0x306A02B7,
    0xE226404C, 0x2C7E82FD, 0x4C57C3B7, 0x820F0106, 0x8C054123, 0x425D8392, 0x2274C2D8, 0xEC2C0069,
    0xB42C41B7, 0x7A748306, 0x1A5DC24C, 0xD40500FD, 0xDA0F40D8, 0x14578269, 0x747EC323, 0xBA260192,
    0x686A4369, 0xA63281D8, 0xC61BC092, 0x08430223, 0x06494206, 0xC81180B7, 0xA838C1FD, 0x6660034C,
    0xCDE840AB, 0x03B0821A, 0x6399C350, 0xADC101E1, 0xA3CB41C4, 0x6D938375, 0x0DBAC23F, 0xC3E2008E,
    0x11AE4275, 0xDFF680C4, 0xBFDFC18E, 0x7187033F, 0x7F8D431A, 0xB1D581AB, 0xD1FCC0E1, 0x1FA40250,
    0x47A4438E, 0x89FC813F, 0xE9D5C075, 0x278D02C4, 0x298742E1, 0xE7DF8050, 0x87F6C11A, 0x49AE03AB,
    0x9BE24150, 0x55BA83E1, 0x3593C2AB, 0xFBCB001A, 0xF5C1403F, 0x3B99828E, 0x5BB0C3C4, 0x95E80175,
    0xEBB04078, 0x25E882C9, 0x45C1C383, 0x8B990132, 0x85934117, 0x4BCB83A6, 0x2BE2C2EC, 0xE5BA005D,
    0x37F642A6, 0xF9AE8017, 0x9987C15D, 0x57DF03EC, 0x59D543C9, 0x978D8178, 0xF7A4C032, 0x39FC0283,
    0x61FC435D, 0xAFA481EC, 0xCF8DC0A6, 0x01D50217, 0x0FDF4232, 0xC1878083, 0xA1AEC1C9, 0x6FF60378,
    0xBDBA4183, 0x73E28332, 0x13CBC278, 0xDD9300C9, 0xD39940EC, 0x1DC1825D, 0x7DE8C317, 0xB3B001A6,
    0x8158410D, 0x4F0083BC, 0x2F29C2F6, 0xE1710047, 0xEF7B4062, 0x212382D3, 0x410AC399, 0x8F520128,
    0x5D1E43D3, 0x93468162, 0xF36FC028, 0x3D370299, 0x333D42BC, 0xFD65800D, 0x9D4CC147, 0x531403F6,
    0x0B144228, 0xC54C8099, 0xA565C1D3, 0x6B3D0362, 0x65374347, 0xAB6F81F6, 0xCB46C0BC, 0x051E020D,
    0xD75240F6, 0x190A8247, 0x7923C30D, 0xB77B01BC, 0xB9714199, 0x77298328, 0x1700C262, 0xD95800D3,
    0xA70041DE, 0x6958836F, 0x0971C225, 0xC7290094, 0xC92340B1, 0x077B8200, 0x6752C34A, 0xA90A01FB,
    0x7B464300, 0xB51E81B1, 0xD537C0FB, 0x1B6F024A, 0x1565426F, 0xDB3D80DE, 0xBB14C194, 0x754C0325,
    0x2D4C42FB, 0xE314804A, 0x833DC100, 0x4D6503B1, 0x436F4394, 0x8D378125, 0xED1EC06F, 0x234602DE,
    0xF10A4025, 0x3F528294, 0x5F7BC3DE, 0x9123016F, 0x9F29414A, 0x517183FB, 0x3158C2B1, 0xFF000000,
];

pub fn compute(initial_crc: u32, data: &[u8]) -> u32 {
    let mut reg = initial_crc;
    for &byte in data.iter() {
        reg = (reg >> 8) ^ PARTIAL_RESULTS[(reg as u8 ^ byte) as usize];
    }
    return reg;
}

#[cfg(test)]
mod tests {
    use super::*;

    /*
    #[test]
    fn main() {
        println!("polynomial: {:b}", 0x132c00699u64);

        let check_value = compute_slow(0, "123456789".as_bytes().into());
        println!("check value: {:X}", check_value);

        let mut code: usize = 0;
        for _ in 0 .. 32 {
            for _ in 0 .. 8 {
                print!("0x{:08X}, ", compute_slow(0, &[code as u8]));
                code += 1;
            }
            println!("");
        }
    }
    */

    #[test]
    fn zero_nonzero_crc() {
        assert!(compute_slow(0, &[0]) != 0);
    }

    #[test]
    fn basic() {
        assert_eq!(compute_slow(0, "123456789".as_bytes().into()), 0x11A6F2A3);
        assert_eq!(compute(0, "123456789".as_bytes().into()), 0x11A6F2A3);
    }

    #[test]
    fn random() {
        for _ in 0 .. 100 {
            let data = (0 .. 1024).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice();
            let initial_crc = rand::random::<u32>();
            assert_eq!(compute_slow(initial_crc, &data), compute(initial_crc, &data));
        }
    }
}

