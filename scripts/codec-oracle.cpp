// Test-only driver. message.inc contains unmodified functions extracted from
// the pinned OpenJK source by build-codec-reference.py.
#include "qcommon/qcommon.h"
#include "message.inc"
#include "entity.inc"
#include <cstdint>
#include <cstdio>
#include <fstream>
#include <map>
#include <vector>

static void dump_gamestate(const char* path) {
    std::ifstream file(path, std::ios::binary);
    int sequence, length;
    if (!file.read(reinterpret_cast<char*>(&sequence), 4) || !file.read(reinterpret_cast<char*>(&length), 4) || length < 0 || length > 49152)
        throw std::runtime_error("bad demo record");
    std::vector<byte> data(length);
    if (!file.read(reinterpret_cast<char*>(data.data()), length)) throw std::runtime_error("truncated record");
    msg_t msg{};
    msg.data = data.data();
    msg.cursize = length;
    const int ack = MSG_ReadLong(&msg);
    auto read_text = [&](size_t limit) {
        std::vector<byte> text;
        for (;;) {
            int ch = MSG_ReadByte(&msg);
            if (ch < 0 || text.size() >= limit) throw std::runtime_error("string bounds");
            if (!ch) return text;
            text.push_back(ch);
        }
    };
    std::vector<std::pair<int, std::vector<byte>>> preceding;
    for (;;) {
        const int opcode = MSG_ReadByte(&msg);
        if (opcode == 2) break;
        if (opcode == 1) continue;
        if (opcode != 5 || preceding.size() >= 128) throw std::runtime_error("not initial gamestate");
        const int sequence = MSG_ReadLong(&msg);
        preceding.emplace_back(sequence, read_text(1024));
    }
    const int commands = MSG_ReadLong(&msg);
    std::map<int, std::vector<byte>> strings;
    std::map<int, entityState_t> baselines;
    for (;;) {
        const int opcode = MSG_ReadByte(&msg);
        if (opcode == 10) break;
        if (opcode == 3) {
            int index = MSG_ReadShort(&msg);
            if (index < 0 || index >= 1700) throw std::runtime_error("configstring index");
            std::vector<byte> text;
            for (;;) {
                int ch = MSG_ReadByte(&msg);
                if (ch < 0 || text.size() >= 8192) throw std::runtime_error("string bounds");
                if (!ch) break;
                text.push_back(ch); // raw bytes before engine string sanitization
            }
            strings[index] = text;
        } else if (opcode == 4) {
            const int number = MSG_ReadBits(&msg, 10);
            entityState_t zero{}, result{};
            MSG_ReadDeltaEntity(&msg, &zero, &result, number);
            if (msg.readcount > length) throw std::runtime_error("truncated entity");
            baselines[number] = result;
        } else throw std::runtime_error("gamestate opcode");
    }
    const int client = MSG_ReadLong(&msg);
    const uint32_t checksum = MSG_ReadLong(&msg);
    if (MSG_ReadShort(&msg) != 0 || MSG_ReadByte(&msg) != 10) throw std::runtime_error("unsupported gamestate tail");
    std::printf("meta\t%d\t%d\t%d\t%d\t%u\t%d\t%d\n", sequence, ack, commands, client, checksum, msg.bit, msg.readcount);
    for (const auto& command : preceding) {
        std::printf("cmd\t%d\t", command.first);
        for (byte value : command.second) std::printf("%02x", value);
        std::puts("");
    }
    for (const auto& entry : strings) {
        std::printf("cs\t%d\t", entry.first);
        for (byte value : entry.second) std::printf("%02x", value);
        std::puts("");
    }
    for (const auto& entry : baselines) {
        std::printf("base\t%d", entry.first);
        for (uint32_t value : entry.second.fields) std::printf("\t%08x", value);
        std::puts("");
    }
}

int main(int argc, char** argv) {
    static_assert(sizeof(int) == 4 && sizeof(short) == 2);
    const uint16_t endian = 1;
    if (*reinterpret_cast<const byte*>(&endian) != 1) return 2;
    MSG_initHuffman();
    if (argc == 2) {
        try { dump_gamestate(argv[1]); return 0; }
        catch (const std::exception& error) { std::fprintf(stderr, "%s\n", error.what()); return 1; }
    }
    for (int symbol = 0; symbol < 256; ++symbol) {
        byte data[8] = {};
        int bits = 0;
        Huff_offsetTransmit(&msgHuff.compressor, symbol, data, &bits, 64);
        std::printf("code\t%d\t%d\t%d\n", symbol, data[0] | (data[1] << 8), bits);
    }
    // Every field width and starting bit alignment, both conventional signed
    // widths and the unusual negative-width behavior of the actual reference.
    for (int prefix = 0; prefix < 8; ++prefix) {
        for (int width = 1; width <= 32; ++width) {
            const uint32_t mask = width == 32 ? UINT32_MAX : (uint32_t(1) << width) - 1;
            const uint32_t values[] = {0, 1, mask, 0xa5c39e71u & mask};
            for (uint32_t value : values) {
                for (int sign = 0; sign < (width == 32 ? 1 : 2); ++sign) {
                    byte data[128] = {};
                    msg_t out{};
                    out.data = data;
                    out.maxsize = sizeof(data);
                    if (prefix) MSG_WriteBits(&out, 0x55 & ((1 << prefix) - 1), prefix);
                    MSG_WriteBits(&out, static_cast<int>(value), width);
                    MSG_WriteBits(&out, 10, 8);
                    msg_t in{};
                    in.data = data;
                    in.cursize = out.cursize;
                    if (prefix) MSG_ReadBits(&in, prefix);
                    const int readWidth = sign ? -width : width;
                    const int expected = MSG_ReadBits(&in, readWidth);
                    if (MSG_ReadBits(&in, 8) != 10 || in.bit != out.bit) return 3;
                    std::printf("vector\t%d\t%d\t%u\t%d\t%d\t%d\t", prefix, readWidth, value, expected, out.bit, out.cursize);
                    for (int i = 0; i < out.cursize; ++i) std::printf("%02x", data[i]);
                    std::puts("");
                }
            }
        }
    }
    // Synthetic fixture: two float encodings and an integer baseline field.
    byte data[1024] = {};
    msg_t msg{};
    msg.data = data;
    msg.maxsize = sizeof(data);
    auto w = [&](int value, int bits) { MSG_WriteBits(&msg, value, bits); };
    w(123, 32); w(1, 8); w(5, 8); w(450, 32);
    const char* command = "print ready";
    do { w(*command, 8); } while (*command++);
    w(2, 8); w(456, 32); w(3, 8); w(0, 16);
    const char* text = "\\mapname\\mp/ffa3";
    do { w(*text, 8); } while (*text++);
    w(4, 8); w(7, 10); w(0, 1); w(1, 1); w(3, 8);
    w(1, 1); w(1, 1); w(1234, 32);
    w(1, 1); w(1, 1); w(0, 1); w(4094, 13);
    w(1, 1); w(1, 1); w(1, 1); w(0x3fc00000, 32);
    w(10, 8); w(2, 32); w(static_cast<int>(0x89abcdefu), 32); w(0, 16); w(10, 8);
    std::printf("gamestate\t");
    for (int i = 0; i < msg.cursize; ++i) std::printf("%02x", data[i]);
    std::puts("");
}
