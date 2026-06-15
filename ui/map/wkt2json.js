// https://gitea.osgeo.org/postgis/postgis/src/branch/master/doc/bnf-wkt.txt
// https://github.com/postgis/postgis/blob/e936c74a8139d68beb081c2c55c131e8aa6d16e8/liblwgeom/lwin_wkt_parse.y
// https://github.com/postgis/postgis/blob/e936c74a8139d68beb081c2c55c131e8aa6d16e8/liblwgeom/lwin_wkt_lex.l

export function wkt2json(/** @type {string} */ inp) {
  let pos = 0;

  const srid = eat(/srid=[^;]*;/yig)?.slice(5, -1);
  return {
    type: /** @type {const} */ ('GeometryCollection'),
    geometries: read_geometry(),
    crs: !srid ? undefined : {
      type: /** @type {const} */ ('name'),
      properties: { name: `urn:ogc:def:crs:EPSG::${srid}` },
    },
  };

  function read_geometry() {
    // TODO empty geom
    if (eat(/point\s*z?m?/yig)) return [{ type: 'Point', coordinates: read_path()[0] }];
    if (eat(/linestring\s*z?m?/yig)) return [{ type: 'LineString', coordinates: read_path() }];
    if (eat(/polygon\s*z?m?/yig)) return [{ type: 'Polygon', coordinates: read_path2() }];
    if (eat(/multipoint\s*z?m?/yig)) return read_path().map(coordinates => ({ type: 'Point', coordinates }));
    if (eat(/multilinestring\s*z?m?/yig)) return read_path2().map(coordinates => ({ type: 'LineString', coordinates }));
    if (eat(/multipolygon\s*z?m?/yig)) return read_array(read_path2).map(coordinates => ({ type: 'Polygon', coordinates }));
    if (eat(/geometrycollection\s*z?m?/yig)) return read_array(read_geometry).flat();
    throw Error('WKT type not supported', { cause: { pos } });
  }

  function read_path2() {
    return read_array(read_path);
  }

  function read_path() {
    return read_array(read_xyzm);
  }

  /** @template T */
  function read_array(/** @type {() => T} */ read_elem) {
    if (!eat(/\(/yg)) throw Error('WKT malformed', { cause: { pos } });
    for (const array = [];;) {
      array.push(read_elem());
      if (eat(/,/yg)) continue;
      if (eat(/\)/yg)) return array;
      throw Error('WKT malformed', { cause: { pos } });
    }
  }

  function read_xyzm() {
    const vertex = [];
    // TODO more strict number?
    // https://github.com/postgis/postgis/blob/e936c74a8139d68beb081c2c55c131e8aa6d16e8/liblwgeom/lwin_wkt_lex.l#L62
    for (let n; (n = eat(/[-+\d.ena]+/yig));) {
      vertex.push(Number(n));
    }
    // TODO check vertex.length > 1
    return vertex;
  }

  function eat(/** @type {RegExp} */ p) {
    // TODO I see no performance gain when moving regexp initialization out of loop
    // https://github.com/postgis/postgis/blob/e936c74a8139d68beb081c2c55c131e8aa6d16e8/liblwgeom/lwin_wkt_lex.l#L110
    const ws = /[ \t\n\r]*/yg;
    ws.lastIndex = pos;
    ws.test(inp);

    p.lastIndex = ws.lastIndex;
    if (!p.test(inp)) return null;
    pos = p.lastIndex;
    return inp.slice(ws.lastIndex, p.lastIndex);
  }
}
