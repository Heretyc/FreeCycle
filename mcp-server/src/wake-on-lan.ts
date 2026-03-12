import { createSocket } from "node:dgram";
import { WakeOnLanConfig } from "./config.js";

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function normalizeMacAddress(macAddress: string): Buffer {
  const sanitized = macAddress.replace(/[^0-9A-Fa-f]/g, "");
  if (sanitized.length !== 12) {
    throw new Error(
      "wakeOnLan.macAddress must contain exactly 12 hexadecimal characters.",
    );
  }

  const bytes = Buffer.alloc(6);
  for (let index = 0; index < 6; index += 1) {
    const pair = sanitized.slice(index * 2, index * 2 + 2);
    bytes[index] = Number.parseInt(pair, 16);
  }

  return bytes;
}

function createMagicPacket(macBytes: Buffer): Buffer {
  const packet = Buffer.alloc(6 + 16 * macBytes.length, 0xff);
  for (let repeat = 0; repeat < 16; repeat += 1) {
    macBytes.copy(packet, 6 + repeat * macBytes.length);
  }
  return packet;
}

function sendPacket(
  packet: Buffer,
  broadcastAddress: string,
  port: number,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const socket = createSocket("udp4");
    socket.once("error", (error) => {
      socket.close();
      reject(error);
    });

    socket.bind(() => {
      socket.setBroadcast(true);
      socket.send(packet, port, broadcastAddress, (error) => {
        socket.close();
        if (error != null) {
          reject(error);
          return;
        }

        resolve();
      });
    });
  });
}

export async function sendWakeOnLanPackets(
  config: WakeOnLanConfig,
): Promise<void> {
  const macBytes = normalizeMacAddress(config.macAddress);
  const packet = createMagicPacket(macBytes);

  for (let sent = 0; sent < config.packetCount; sent += 1) {
    await sendPacket(packet, config.broadcastAddress, config.port);
    if (sent < config.packetCount - 1) {
      await sleep(config.packetIntervalMs);
    }
  }
}
