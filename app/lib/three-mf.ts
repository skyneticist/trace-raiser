import { strToU8, zipSync } from "fflate";

function formatNumber(value: number) {
  return Number.isFinite(value) ? value.toFixed(5).replace(/\.?0+$/, "") : "0";
}

export function stlTo3mf(stl: Uint8Array, title: string): Uint8Array {
  if (stl.byteLength < 84) {
    throw new Error("The generated STL is incomplete.");
  }

  const view = new DataView(stl.buffer, stl.byteOffset, stl.byteLength);
  const triangleCount = view.getUint32(80, true);
  const expectedLength = 84 + triangleCount * 50;
  if (stl.byteLength < expectedLength) {
    throw new Error("The generated STL contains truncated triangle data.");
  }

  const vertices: Array<[number, number, number]> = [];
  const triangles: Array<[number, number, number]> = [];
  const vertexIndex = new Map<string, number>();

  const intern = (x: number, y: number, z: number) => {
    const key = `${x.toFixed(5)},${y.toFixed(5)},${z.toFixed(5)}`;
    const existing = vertexIndex.get(key);
    if (existing !== undefined) return existing;
    const index = vertices.length;
    vertices.push([x, y, z]);
    vertexIndex.set(key, index);
    return index;
  };

  for (let triangle = 0; triangle < triangleCount; triangle += 1) {
    const offset = 84 + triangle * 50 + 12;
    const ids = [0, 1, 2].map((vertex) => {
      const vertexOffset = offset + vertex * 12;
      return intern(
        view.getFloat32(vertexOffset, true),
        view.getFloat32(vertexOffset + 4, true),
        view.getFloat32(vertexOffset + 8, true),
      );
    });
    triangles.push(ids as [number, number, number]);
  }

  const model = `<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <metadata name="Title">${escapeXml(title)}</metadata>
  <metadata name="Application">Copperline Studio</metadata>
  <resources>
    <object id="1" name="${escapeXml(title)}" type="model">
      <mesh>
        <vertices>
${vertices.map(([x, y, z]) => `          <vertex x="${formatNumber(x)}" y="${formatNumber(y)}" z="${formatNumber(z)}"/>`).join("\n")}
        </vertices>
        <triangles>
${triangles.map(([v1, v2, v3]) => `          <triangle v1="${v1}" v2="${v2}" v3="${v3}"/>`).join("\n")}
        </triangles>
      </mesh>
    </object>
  </resources>
  <build><item objectid="1"/></build>
</model>`;

  return zipSync(
    {
      "[Content_Types].xml": strToU8(
        `<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>`,
      ),
      "_rels/.rels": strToU8(
        `<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>`,
      ),
      "3D/3dmodel.model": strToU8(model),
    },
    { level: 6 },
  );
}

function escapeXml(value: string) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&apos;");
}
