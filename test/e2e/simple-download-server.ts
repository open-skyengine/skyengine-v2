import { createHash } from "node:crypto";
import { createServer, type Server, type Socket } from "node:net";

export interface SimpleDownloadServer {
  readonly port: number;
  readonly requests: Buffer[];
  close(): Promise<void>;
}

function uint16(value: number): Buffer {
  const result = Buffer.alloc(2);
  result.writeUInt16BE(value);
  return result;
}

function uint32(value: number): Buffer {
  const result = Buffer.alloc(4);
  result.writeUInt32BE(value);
  return result;
}

function tlv(tag: number, value: Buffer): Buffer {
  return Buffer.concat([uint32(tag), uint32(value.length), value]);
}

function parseTlvFields(body: Buffer): Map<number, Buffer> {
  const fields = new Map<number, Buffer>();
  for (let offset = 0; offset < body.length;) {
    if (body.length - offset < 8) throw new Error("truncated simpleDownload TLV header");
    const tag = body.readUInt32BE(offset);
    const length = body.readUInt32BE(offset + 4);
    offset += 8;
    if (length > body.length - offset) throw new Error("truncated simpleDownload TLV value");
    fields.set(tag, body.subarray(offset, offset + length));
    offset += length;
  }
  return fields;
}

function requiredUInt32(fields: Map<number, Buffer>, tag: number): number {
  const value = fields.get(tag);
  if (value?.length !== 4) {
    throw new Error(`simpleDownload field ${tag} is missing or malformed`);
  }
  return value.readUInt32BE();
}

export function simpleDownloadRequestAppId(request: Buffer): number {
  const headerEnd = request.indexOf("\r\n\r\n");
  if (headerEnd < 0) throw new Error("simpleDownload request has no HTTP header terminator");
  return requiredUInt32(parseTlvFields(request.subarray(headerEnd + 4)), 0x29ce);
}

function buildSimpleDownloadResponse(request: Buffer, plugin: Buffer): Buffer {
  const headerEnd = request.indexOf("\r\n\r\n");
  if (headerEnd < 0) throw new Error("simpleDownload request has no HTTP header terminator");
  const fields = parseTlvFields(request.subarray(headerEnd + 4));
  const productId = requiredUInt32(fields, 0x2775);
  const appId = requiredUInt32(fields, 0x29ce);
  const metadata = Buffer.concat([
    tlv(0x2c7, uint32(1)),
    tlv(0x2be, uint32(appId)),
    tlv(0x2bf, uint32(0)),
    tlv(0x2c0, Buffer.alloc(0)),
    tlv(0x2c1, Buffer.alloc(0)),
    tlv(0x2c2, uint16(0)),
    tlv(0x2c3, uint32(0)),
    tlv(0x2c4, uint32(plugin.length)),
    tlv(0x2c5, createHash("md5").update(plugin).digest()),
    tlv(0x2c6, uint32(0)),
  ]);
  return Buffer.concat([
    tlv(0x64, uint32(200)),
    tlv(0x65, uint32(productId)),
    tlv(0x6d, Buffer.from("00000000000000000001", "ascii")),
    tlv(0x2bc, uint16(1)),
    tlv(0x2bd, metadata),
    tlv(0x2c3, uint32(0)),
    tlv(0x2d1, plugin),
  ]);
}

export async function startSimpleDownloadServer(plugin: Buffer): Promise<SimpleDownloadServer> {
  const requests: Buffer[] = [];
  const sockets = new Set<Socket>();
  const server: Server = createServer(socket => {
    sockets.add(socket);
    let buffered = Buffer.alloc(0);
    let responded = false;
    socket.on("data", chunk => {
      if (responded) return;
      buffered = Buffer.concat([buffered, chunk]);
      const headerEnd = buffered.indexOf("\r\n\r\n");
      if (headerEnd < 0) return;
      const headers = buffered.subarray(0, headerEnd).toString("latin1");
      const contentLength = /^Content-Length:\s*(\d+)$/im.exec(headers);
      if (!contentLength) {
        socket.destroy();
        return;
      }
      const requestLength = headerEnd + 4 + Number(contentLength[1]);
      if (buffered.length < requestLength) return;
      responded = true;
      const request = Buffer.from(buffered.subarray(0, requestLength));
      requests.push(request);
      try {
        const responseBody = buildSimpleDownloadResponse(request, plugin);
        const responseHead = Buffer.from(
          `HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nContent-Length: ${responseBody.length}\r\nConnection: close\r\n\r\n`,
          "ascii",
        );
        socket.end(Buffer.concat([responseHead, responseBody]));
      } catch {
        socket.destroy();
      }
    });
    socket.on("error", () => {});
    socket.on("close", () => sockets.delete(socket));
  });

  await new Promise<void>((resolve, reject) => {
    const onError = (error: Error) => reject(error);
    server.once("error", onError);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", onError);
      resolve();
    });
  });
  const address = server.address();
  if (!address || typeof address === "string") {
    server.close();
    throw new Error("simpleDownload server did not expose a TCP port");
  }

  return {
    port: address.port,
    requests,
    async close() {
      for (const socket of sockets) socket.destroy();
      if (!server.listening) return;
      await new Promise<void>((resolve, reject) => {
        server.close(error => error ? reject(error) : resolve());
      });
    },
  };
}

export function simpleDownloadDnsMap(
  server: SimpleDownloadServer,
  extra: string[] = [],
): string {
  return [
    `10.0.0.172->127.0.0.1:${server.port}`,
    `spd.skymobiapp.com->127.0.0.1:${server.port}`,
    ...extra,
  ].join(";");
}
