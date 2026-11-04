// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { detectKind, firstLine, lineElevations, lineToGpx, parseGeoFile, parseGeoJson, parseGpx, parseKml } from "./geoFiles";

const GPX = `<?xml version="1.0"?>
<gpx version="1.1" creator="t" xmlns="http://www.topografix.com/GPX/1/1">
  <wpt lat="32.1" lon="34.8"><name>מעיין</name></wpt>
  <trk><name>שביל</name>
    <trkseg>
      <trkpt lat="32.0" lon="34.7"><ele>10</ele></trkpt>
      <trkpt lat="32.01" lon="34.71"><ele>25.5</ele></trkpt>
    </trkseg>
    <trkseg>
      <trkpt lat="32.02" lon="34.72"/>
      <trkpt lat="32.03" lon="34.73"/>
      <trkpt lat="32.04" lon="34.74"/>
    </trkseg>
  </trk>
  <rte><rtept lat="31.0" lon="35.0"/><rtept lat="31.1" lon="35.1"/></rte>
</gpx>`;

const KML = `<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2">
<Document>
  <Placemark><name>Point A</name><Point><coordinates>34.8,32.1,0</coordinates></Point></Placemark>
  <Placemark><name>Line</name><LineString><coordinates>34.7,32.0,5 34.71,32.01,7</coordinates></LineString></Placemark>
  <Placemark><name>Area</name><Polygon><outerBoundaryIs><LinearRing><coordinates>34,32 34.1,32 34.1,32.1 34,32</coordinates></LinearRing></outerBoundaryIs></Polygon></Placemark>
  <Placemark><name>Multi</name><MultiGeometry><Point><coordinates>35,31</coordinates></Point><LineString><coordinates>35,31 35.1,31.1</coordinates></LineString></MultiGeometry></Placemark>
  <Placemark><name>Track</name><gx:Track><gx:coord>34.5 31.5 100</gx:coord><gx:coord>34.6 31.6 110</gx:coord></gx:Track></Placemark>
</Document></kml>`;

describe("GPX", () => {
  it("reads tracks (multi-segment), routes and waypoints with elevation", () => {
    const fc = parseGpx(GPX);
    expect(fc.features.map((f) => f.geometry.type)).toEqual(["MultiLineString", "LineString", "Point"]);
    const trk = fc.features[0];
    expect(trk.properties?.name).toBe("שביל");
    if (trk.geometry.type !== "MultiLineString") throw new Error();
    expect(trk.geometry.coordinates[0][1]).toEqual([34.71, 32.01, 25.5]);
    expect(trk.geometry.coordinates[1][0]).toEqual([34.72, 32.02]);
    expect(fc.features[2].properties?.name).toBe("מעיין");
  });
  it("first line = the longest part, elevations kept", () => {
    const fc = parseGpx(GPX);
    expect(firstLine(fc)).toEqual([
      [34.72, 32.02],
      [34.73, 32.03],
      [34.74, 32.04],
    ]);
    expect(lineElevations(fc)?.slice(0, 2)).toEqual([10, 25.5]);
  });
  it("round-trips through lineToGpx", () => {
    const pts: [number, number][] = [
      [34.7, 32.0],
      [34.8, 32.1],
    ];
    const back = parseGpx(lineToGpx("a & <b>", pts, [1, null]));
    expect(back.features[0].properties?.name).toBe("a & <b>");
    expect(firstLine(back)).toEqual(pts);
  });
});

describe("KML", () => {
  it("reads points, lines, polygons, multi-geometry and gx:Track", () => {
    const fc = parseKml(KML);
    expect(fc.features.map((f) => f.geometry.type)).toEqual(["Point", "LineString", "Polygon", "Point", "LineString", "LineString"]);
    expect(fc.features[1].geometry).toEqual({ type: "LineString", coordinates: [[34.7, 32.0, 5], [34.71, 32.01, 7]] });
    expect(fc.features[0].geometry).toEqual({ type: "Point", coordinates: [34.8, 32.1] });
    expect(fc.features[5].geometry).toEqual({ type: "LineString", coordinates: [[34.5, 31.5, 100], [34.6, 31.6, 110]] });
  });
});

describe("GeoJSON", () => {
  it("accepts a collection, a feature or a bare geometry", () => {
    expect(parseGeoJson('{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[1,2]}}]}').features).toHaveLength(1);
    expect(parseGeoJson('{"type":"Feature","geometry":{"type":"Point","coordinates":[1,2]}}').features[0].properties).toEqual({});
    expect(parseGeoJson('{"type":"LineString","coordinates":[[1,2],[3,4]]}').features[0].geometry.type).toBe("LineString");
  });
  it("rejects junk with a PARSE error", () => {
    expect(() => parseGeoJson("not json")).toThrow(/^PARSE/);
    expect(() => parseGeoFile("x.txt", "hello")).toThrow(/^PARSE/);
  });
});

describe("detectKind", () => {
  it("by extension, else by content", () => {
    expect(detectKind("a.GPX", "")).toBe("gpx");
    expect(detectKind("a.kml", "")).toBe("kml");
    expect(detectKind("a.json", "")).toBe("geojson");
    expect(detectKind("noext", "  <gpx version=")).toBe("gpx");
    expect(detectKind("noext", "{}")).toBe("geojson");
    expect(detectKind("noext", "hello")).toBeNull();
  });
});
