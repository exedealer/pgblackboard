import maplibregl from '../_vendor/maplibre.js';
import { wkt2json } from './wkt2json.js';
import ne_cities from './ne_cities.json' with { type: 'json' };
import ne_land from './ne_land.json' with { type: 'json' };
import glyphs from './glyphs.json' with { type: 'json' };

const MapGL = maplibregl.Map;

const ne_land_blob = new Blob([JSON.stringify(ne_land)], { type: 'application/json' });
const ne_land_url = URL.createObjectURL(ne_land_blob);

const ne_cities_geojson = {
  type: 'FeatureCollection',
  features: ne_cities.map(([lon, lat, name], weight) => ({
    type: 'Feature',
    geometry: { type: 'Point', coordinates: [lon, lat] },
    properties: { name, weight },
  })),
};
const ne_cities_blob = new Blob([JSON.stringify(ne_cities_geojson)], { type: 'application/json' });
const ne_cities_url = URL.createObjectURL(ne_cities_blob);

const glyphs_blob = await fetch(glyphs).then(x => x.blob());
const glyphs_url = URL.createObjectURL(glyphs_blob);

// const drop_white = /*xml*/ `
//   <svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
//     <path fill="#f0f0f0" stroke="#f0f0f0" fill-opacity="0.8" stroke-width="1.5" d="M4.11619 13.9087L4.11618 13.9087C3.71889 12.9491 3.5 11.9 3.5 10.8C3.5 6.22655 7.29492 2.5 12 2.5C16.7051 2.5 20.5 6.22654 20.5 10.8C20.5 11.9 20.2811 12.9491 19.8838 13.9087C19.5776 14.6484 18.9254 15.6255 18.0848 16.6992C17.2516 17.7634 16.2603 18.889 15.3054 19.9196C14.3514 20.9493 13.4382 21.8796 12.7632 22.5526C12.4511 22.8639 12.1901 23.1199 12 23.3049C11.8099 23.1199 11.5489 22.8639 11.2368 22.5526C10.5618 21.8796 9.64859 20.9493 8.69455 19.9196C7.73971 18.889 6.74839 17.7634 5.91519 16.6992C5.07457 15.6255 4.42238 14.6484 4.11619 13.9087ZM12 14.5C13.933 14.5 15.5 12.933 15.5 11C15.5 9.067 13.933 7.5 12 7.5C10.067 7.5 8.5 9.067 8.5 11C8.5 12.933 10.067 14.5 12 14.5Z" />
//   </svg>
// `;

// const drop_black = /*xml*/ `
//   <svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
//     <path fill="#555" stroke="#555" fill-opacity="0.8" stroke-width="1.5" d="M4.11619 13.9087L4.11618 13.9087C3.71889 12.9491 3.5 11.9 3.5 10.8C3.5 6.22655 7.29492 2.5 12 2.5C16.7051 2.5 20.5 6.22654 20.5 10.8C20.5 11.9 20.2811 12.9491 19.8838 13.9087C19.5776 14.6484 18.9254 15.6255 18.0848 16.6992C17.2516 17.7634 16.2603 18.889 15.3054 19.9196C14.3514 20.9493 13.4382 21.8796 12.7632 22.5526C12.4511 22.8639 12.1901 23.1199 12 23.3049C11.8099 23.1199 11.5489 22.8639 11.2368 22.5526C10.5618 21.8796 9.64859 20.9493 8.69455 19.9196C7.73971 18.889 6.74839 17.7634 5.91519 16.6992C5.07457 15.6255 4.42238 14.6484 4.11619 13.9087ZM12 14.5C13.933 14.5 15.5 12.933 15.5 11C15.5 9.067 13.933 7.5 12 7.5C10.067 7.5 8.5 9.067 8.5 11C8.5 12.933 10.067 14.5 12 14.5Z" />
//   </svg>
// `;

const methods = {
  _render() {
    return { tag: 'div', class: 'map' };
  },
  _mounted() {
    this._ml = new MapGL({
      container: this.$el,
      attributionControl: false, // TODO ?
      // maxPitch: 0,
    });
    this._ml.on('click', this.on_map_click);

    this.$watch(
      this.get_ml_style,
      val => this._ml.setStyle(val),
      { immediate: true },
    );

    // this.add_svg_image('drop_white', drop_white);
    // this.add_svg_image('drop_black', drop_black);
    // this.add_svg_image('drop_marker', drop_marker, true);

    globalThis.debug_map = this._ml;
  },
  _unmounted() {
    this._ml.remove();
  },
  _get_broadcast_listeners() {
    return { 'req_map_navigate': this.on_req_map_navigate };
  },

  /** @returns {import('../store/store.js').Store} */
  get_store() {
    return this.$store;
  },

  * get_geomcols() {
    let i = 0;
    for (const [frame_idx, { rows, cols }] of this.get_store().out.frames.entries())
    for (const [col_idx, col] of cols.entries()) {
      if (!col.is_geom) continue;
      yield { frame_idx, rows, col_idx, col, overlay_idx: i++ };
    }
  },

  get_ml_style() {
    const is_dark = this.get_store().is_dark();
    const show_sat = this.get_store().show_sat;
    const original_features = this.$cached(this.get_original_features);
    const modified_features = this.$cached(this.get_modified_features);
    const selected_features = this.$cached(this.get_selected_features);

    console.log({ selected_features });

    const modified_features_ids = modified_features.full.features.map(f => f.properties.id);
    const overlays_hues = Array.from(this.get_geomcols(), ({ col }) => col.hue);

    // first I use array of booleans as more simple structure,
    // but maplibre evaluates layer filters inconsistenlty with global state.
    // and 'at' operation on empty global array causes
    // 'Array index out of bounds: 0 > -1.' (maplibre v5.16.0)
    const overlays_visible = Array.from(
      this.get_geomcols(),
      ({ col }, overlay_idx) => col.show_on_map ? [overlay_idx] : [],
    ).flat();

    const style = {
      version: 8,
      // globe is not good choise. User need to rotate globe
      // to overview all geometries when geometries located in both hemispheres.
      // projection: { type: 'globe' },

      state: {
        is_dark: { default: is_dark },
        modified_features_ids: { default: modified_features_ids },
        overlays_hues: { default: overlays_hues },
        overlays_visible: { default: overlays_visible },
      },

      glyphs: glyphs_url + '#/{fontstack}/{range}',
      // glyphs: 'https://demotiles.maplibre.org/font/{fontstack}/{range}.pbf',
      transition: {
        duration: 0, // instant theme switch
      },
      sources: {
        'ne_land': {
          type: 'geojson',
          data: ne_land_url,
          tolerance: .2,
        },
        'ne_cities': {
          type: 'geojson',
          data: ne_cities_url,
        },
        'sat_esri': {
          type: 'raster',
          tileSize: 256,
          maxzoom: 23,
          tiles: ['https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}'],
        },
        'sat_google': {
          type: 'raster',
          tileSize: 256,
          maxzoom: 22,
          // tiles: Array.from('0123', d => `https://mt${d}.google.com/vt/lyrs=s&x={x}&y={y}&z={z}`),
          tiles: [
            'https://mt0.google.com/vt/lyrs=s&x={x}&y={y}&z={z}',
            'https://mt1.google.com/vt/lyrs=s&x={x}&y={y}&z={z}',
            'https://mt2.google.com/vt/lyrs=s&x={x}&y={y}&z={z}',
            'https://mt3.google.com/vt/lyrs=s&x={x}&y={y}&z={z}',
          ],
        },
        'sat_bing': {
          type: 'raster',
          tileSize: 256,
          // tiles: Array.from('01234567', d => `http://ecn.t${d}.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1`),
          tiles: [
            'https://ecn.t0.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1',
            'https://ecn.t1.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1',
            'https://ecn.t2.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1',
            'https://ecn.t3.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1',
            'https://ecn.t4.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1',
            'https://ecn.t5.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1',
            'https://ecn.t6.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1',
            'https://ecn.t7.tiles.virtualearth.net/tiles/a{quadkey}.jpeg?g=1',
          ],
        },
        // 'graticules': {
        //   type: 'geojson',
        //   data: graticules,
        // },
        'original_features': {
          type: 'geojson',
          data: original_features.full,
          tolerance: .2,
        },
        'original_pins': {
          type: 'geojson',
          data: original_features.pins,
        },
        'modified_features': {
          type: 'geojson',
          data: modified_features.full,
          tolerance: .2,
        },
        'modified_pins': {
          type: 'geojson',
          data: modified_features.pins,
        },
        'highlight': {
          type: 'geojson',
          data: selected_features,
          tolerance: 0, // show all vertices
        },
      },
      layers: [
        // {
        //   id: 'water',
        //   type: 'background',
        //   paint: {
        //     'background-color': 'hsl(0 0% 0% / .1)',
        //   },
        // },
        {
          id: 'land',
          type: 'fill',
          source: 'ne_land',
          paint: {
            'fill-color': ['case', ['global-state', 'is_dark'],
              'hsl(0 0% 20%)', // dark
              'hsl(0 0% 98%)', // light
            ],
          },
        },
        {
          id: 'sat',
          type: 'raster',
          source: 'sat_google',
          paint: {
            'raster-opacity': .5,
          },
          layout: {
            visibility: show_sat ? 'visible' : 'none',
          },
        },

        // {
        //   id: 'graticules',
        //   type: 'line',
        //   source: 'graticules',
        //   paint: {
        //     'line-width': 1,
        //     'line-color': 'hsl(0 0% 50% / .1)',
        //   },
        // },

        {
          id: 'original_fill',
          type: 'fill',
          source: 'original_features',
          filter: [
            'all',
            ['==', ['geometry-type'], 'Polygon'],
            ['!', ['in', ['get', 'id'], ['global-state', 'modified_features_ids']]],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'fill-opacity': .4,
            'fill-color': ['to-color', ['concat', 'hsl(',
              ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
              ' 100% 70%)',
            ]],
          },
        },
        {
          id: 'modified_fill',
          type: 'fill',
          source: 'modified_features',
          filter: [
            'all',
            ['==', ['geometry-type'], 'Polygon'],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'fill-opacity': .4,
            'fill-color': ['to-color', ['concat', 'hsl(',
              ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
              ' 100% 70%)',
            ]],
          },
        },
        {
          id: 'fill_highlighted',
          type: 'fill',
          source: 'highlight',
          filter: [
            'all',
            ['==', ['geometry-type'], 'Polygon'],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'fill-opacity': .2,
            'fill-color': ['case', ['global-state', 'is_dark'],
              'hsl(0 0% 93%)', // dark
              'hsl(0 0% 33%)', // light
            ],
          },
        },
        {
          id: 'original_line',
          type: 'line',
          source: 'original_features',
          filter: [
            'all',
            ['==', ['geometry-type'], 'LineString'],
            ['!', ['in', ['get', 'id'], ['global-state', 'modified_features_ids']]],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'line-width': 2,
            'line-color': ['to-color', ['concat', 'hsl(',
              ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
              ' 100% 70%)',
            ]],
          },
        },
        {
          id: 'modified_line',
          type: 'line',
          source: 'modified_features',
          filter: [
            'all',
            ['==', ['geometry-type'], 'LineString'],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'line-width': 2,
            'line-color': ['to-color', ['concat', 'hsl(',
              ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
              ' 100% 70%)',
            ]],
          },
        },
        {
          id: 'original_pin',
          type: 'circle',
          source: 'original_pins',
          filter: [
            'all',
            ['<=', ['zoom'], ['get', 'zoom']],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'circle-pitch-alignment': 'map',
            'circle-radius': 2,
            'circle-color': ['to-color', ['concat', 'hsl(',
              ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
              ['case', ['global-state', 'is_dark'],
                '  90% 70% / .6)', // dark
                ' 100% 50% / .6)', // light
              ],
            ]],
          },
        },
        {
          id: 'modified_pin',
          type: 'circle',
          source: 'modified_pins',
          filter: [
            'all',
            ['<=', ['zoom'], ['get', 'zoom']],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'circle-pitch-alignment': 'map',
            'circle-radius': 2,
            'circle-color': ['to-color', ['concat', 'hsl(',
              ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
              ['case', ['global-state', 'is_dark'],
                '  90% 70% / .6)', // dark
                ' 100% 50% / .6)', // light
              ],
            ]],
          },
        },
        {
          id: 'hl_path',
          type: 'line',
          source: 'highlight',
          filter: [
            'all',
            ['!=', ['geometry-type'], 'Point'],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'line-width': 1,
            'line-color': ['case', ['global-state', 'is_dark'],
              'hsl(0 0% 93%)', // dark
              'hsl(0 0% 33%)', // light
            ],
          },
        },
        {
          id: 'hl_vertex',
          type: 'circle',
          source: 'highlight',
          filter: [
            'all',
            ['!=', ['geometry-type'], 'Point'],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'circle-radius': 2,
            'circle-color': ['case', ['global-state', 'is_dark'],
              'hsl(0 0% 93%)', // dark
              'hsl(0 0% 33%)', // light
            ],
          },
        },
        {
          id: 'original_point',
          type: 'circle',
          source: 'original_features',
          filter: [
            'all',
            ['==', ['geometry-type'], 'Point'],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
            ['!', ['in', ['get', 'id'], ['global-state', 'modified_features_ids']]],
          ],
          paint: {
            'circle-radius': 2,
            'circle-stroke-width': 1,
            'circle-color': ['to-color', ['concat', 'hsl(',
              ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
              ['case', ['global-state', 'is_dark'],
                '  90% 50% / .6)', // dark
                ' 100% 50% / .6)', // light
              ],
            ]],
            'circle-stroke-color': ['to-color', ['concat', 'hsl(',
              ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
              ['case', ['global-state', 'is_dark'],
                ' 100% 70%)', // dark
                ' 100% 30%)', // light
              ],
            ]],
          },
        },
        {
          id: 'modified_point',
          type: 'circle',
          source: 'modified_features',
          filter: [
            'all',
            ['==', ['geometry-type'], 'Point'],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'circle-radius': 2,
            'circle-stroke-width': 1,
            'circle-color': ['to-color', ['concat', 'hsl(',
              ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
              ['case', ['global-state', 'is_dark'],
                '  90% 50% / .6)', // dark
                ' 100% 50% / .6)', // light
              ],
            ]],
            'circle-stroke-color': ['to-color', ['concat', 'hsl(',
              ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
              ['case', ['global-state', 'is_dark'],
                ' 100% 70%)', // dark
                ' 100% 30%)', // light
              ],
            ]],
          },
        },
        {
          id: 'cities',
          type: 'symbol',
          source: 'ne_cities',
          paint: {
            'text-opacity': .4, // allows features to be visible through city labels
            'text-halo-width': 1,
            'text-color': ['case', ['global-state', 'is_dark'],
              'hsl(0 0% 90%)', // dark
              'hsl(0 0% 30%)', // light
            ],
            'text-halo-color': ['case', ['global-state', 'is_dark'],
              'hsl(0 0% 0%)', // dark
              'hsl(0 0% 100%)', // light
            ],
          },
          layout: {
            'text-size': 14,
            'text-field': ['get', 'name'],
            'text-padding': 8,
            'symbol-sort-key': ['get', 'weight'],
            'text-anchor': 'bottom', // prevent features from being overlapped by city labels
          },
        },
        // TODO 'circle-pitch-alignment': 'map' for collapsed points
        {
          id: 'hl_point',
          type: 'circle',
          source: 'highlight',
          filter: [
            'all',
            ['==', ['geometry-type'], 'Point'],
            // ['<=', ['zoom'], ['coalesce', ['get', 'zoom'], 100]],
            ['in', ['get', 'overlay_idx'], ['global-state', 'overlays_visible']],
          ],
          paint: {
            'circle-radius': 5,
            'circle-stroke-width': 1,
            'circle-color': 'transparent',
            'circle-stroke-color': ['case', ['global-state', 'is_dark'],
              'hsl(0 0% 90%)', // dark
              'hsl(0 0% 30%)', // light
            ],

            // 'circle-radius': 3,
            // 'circle-stroke-width': 1,
            // 'circle-color':'hsl(0 0% 100%)',
            // 'circle-stroke-color': ['to-color', ['concat', 'hsl(',
            //   ['at', ['get', 'overlay_idx'], ['global-state', 'overlays_hues']],
            //   ['case', ['global-state', 'is_dark'],
            //     '  90% 50%)', // dark
            //     ' 100% 50%)', // light
            //   ],
            // ]],
          },
        },
        // {
        //   id: 'hl_point',
        //   type: 'symbol',
        //   filter: ['==', ['geometry-type'], 'Point'],
        //   source: 'highlight',
        //   paint: {
        //     // 'icon-color': '#fff',
        //     // 'icon-halo-color': 'red',
        //     // 'icon-halo-width': 2,
        //   },
        //   layout: {
        //     // 'icon-image': 'drop_marker',
        //     'icon-image': 'drop_white',
        //     'icon-anchor': 'bottom',
        //     'icon-allow-overlap': true,
        //   },
        //   metadata: {
        //     alt_layout: {
        //       'icon-image': 'drop_black',
        //     },
        //   },
        // },
      ],
    };

    return style;
  },

  // TODO split primary and collapsed features to different featurecollections
  /**
  * @param {'original' | 'modified'} original_or_modified
  * @returns
  */
  get_features(original_or_modified) {
    const granularity = 4 /*marker diameter px*/ / 512 /*world size px*/;
    const features = [], pins = [], dict = new Map();
    for (const { overlay_idx, frame_idx, rows, col_idx } of this.get_geomcols())
    for (let row_idx = rows.length; row_idx--;) {
      // TODO bench original_or_modified static vs dynamic dispatch
      const wkt = rows[row_idx][original_or_modified]?.[col_idx];
      if (wkt === undefined) continue;
      let geometry;
      try {
        geometry = wkt2json(wkt);
      } catch { }

      const id = `${overlay_idx}.${row_idx}`;
      const dict_features = [];
      dict.set(id, dict_features);

      if (!geometry) continue;

      for (const singular_geom of geometry.geometries) {
        const [w, s, e, n] = geojson_bbox(singular_geom);

        const f = {
          type: 'Feature',
          properties: { frame_idx, row_idx, overlay_idx, id },
          geometry: singular_geom,
        };
        features.push(f);
        dict_features.push(f);

        // geojson_extend_bbox(bbox, w, s);
        // geojson_extend_bbox(bbox, e, n);
        if (singular_geom.type == 'Point') continue;

        const span = Math.hypot((e - w) / 360, (s - n) / 180); // TODO mercator
        const zoom = Math.log2(granularity / span);
        const coordinates = [(w + e) * .5, (s + n) * .5];
        pins.push({
          type: 'Feature',
          properties: { frame_idx, row_idx, overlay_idx, zoom },
          geometry: { type: 'Point', coordinates },
        });
      }
    }
    return {
      full: { type: 'FeatureCollection', features },
      pins: { type: 'FeatureCollection', features: pins },
      dict,
    };
  },

  get_original_features() {
    return this.get_features('original');
  },

  get_modified_features() {
    return this.get_features('modified');
  },

  // TODO 3 levels of highlight
  // when row focused
  // when geom cell is focused
  // when vertex selected
  get_selected_features() {
    const { frame_idx, row_idx } = this.get_store().get_selected_rowcol();
    // const { cols } = this.get_store().out.frames[frame_idx];
    const original = this.$cached(this.get_original_features);
    const modified = this.$cached(this.get_modified_features);
    const features = [];
    for (const g of this.get_geomcols()) {
      if (g.frame_idx != frame_idx) continue;
      const feature_id = `${g.overlay_idx}.${row_idx}`;
      // TODO avoid null features
      features.push(
        ... modified.dict.get(feature_id)
        || original.dict.get(feature_id)
        || []
      );
    }
    return { type: 'FeatureCollection', features };
  },

  // add_svg_image(id, svg, sdf) {
  //   this._ml.addImage(id, { data: [0, 0, 0, 0], width: 1, height: 1 });
  //   const img = new Image();
  //   img.src = 'data:image/svg+xml,' + encodeURIComponent(svg);
  //   img.onload = _ => {
  //     img.width *= 2;
  //     img.height *= 2;
  //     this._ml.removeImage(id);
  //     this._ml.addImage(id, img, { pixelRatio: 2, sdf });
  //   };
  // },

  on_map_click({ point }) {
    const pad = { x: 2, y: 2 };
    const qbox = [point.sub(pad), point.add(pad)];
    const feature = (
      // TODO exclude already highlighted feature
      this._ml.queryRenderedFeatures(qbox, {
        layers: [
          'original_point',
          'original_pin',
          'original_line',
          'modified_point',
          'modified_pin',
          'modified_line',
        ],
      })[0] ||
      this._ml.queryRenderedFeatures(point, {
        layers: [
          'original_fill',
          'modified_fill',
        ],
      })[0]
    );
    if (!feature) return; // TODO clear highlight
    const { frame_idx, row_idx } = feature.properties;
    this.$store.set_selected_rowcol(frame_idx, row_idx);
    this.$broadcast('req_row_navigate');
    // TODO zoom to feature extent
  },

  on_req_map_navigate() {
    return; // TODO make navigation unobtrusive

    // const hl_fcoll = this.$cached(this.get_selected_features);
    // const feature = hl_fcoll.features.find(f => f.bbox);
    // if (!feature) return;
    // // TODO globe support (and pitch, bearing)
    // // const padding = 0; // px
    // const { width, height } = this._ml.transform;
    // // const sw = this._ml.unproject([padding, height - padding]);
    // // const ne = this._ml.unproject([width - padding, padding]);
    // // const bounds = new LngLatBounds(sw, ne);
    // // const bounds = this._ml.getBounds();
    // // bounds.extend(feature.bbox);
    // const padding = {
    //   left: width * .4,
    //   right: width * .4,
    //   top: height * .4,
    //   bottom: height * .4,
    // };
    // this._ml.fitBounds(feature.bbox, { padding });
    // // TODO if point then use box of the point and nearest point from dataset
  },
};

function geojson_bbox({ type, coordinates }) {
  switch (type) {
    case 'Point':
      coordinates = [coordinates];
      break;
    case 'Polygon':
      coordinates = coordinates[0]; // take exterior ring only
      break;
    case 'MultiPoint':
    case 'LineString':
      break;
    default:
      throw Error('impossible');
  }
  const bbox = [180, 90, -180, -90];
  for (const p of coordinates) {
    // [lng, lat] = p // 3x slower
    geojson_extend_bbox(bbox, p[0], p[1]);
  }
  return bbox;
}

function geojson_extend_bbox(bbox, lng, lat) {
  // TODO world wrap
  bbox[0] = Math.min(bbox[0], lng);
  bbox[1] = Math.min(bbox[1], lat);
  bbox[2] = Math.max(bbox[2], lng);
  bbox[3] = Math.max(bbox[3], lat);
}

export default { methods };


// const graticules = {
//   type: 'FeatureCollection',
//   features: [],
// };

// const minlat = -85, maxlat = 85;
// const minlon = -180, maxlon = 180;

// for (let lon = minlon; lon < maxlon; lon++) {
//   graticules.features.push({
//     type: 'Feature',
//     properties: { value: lon },
//     geometry: {
//       type: 'LineString',
//       coordinates: [[lon, minlat], [lon, maxlat]],
//     },
//   });
// }

// for (let lat = minlat; lat < maxlat; lat++) {
//   graticules.features.push({
//     type: 'Feature',
//     properties: { value: lat },
//     geometry: {
//       type: 'LineString',
//       coordinates: [[minlon, lat], [maxlon, lat]],
//     },
//   });
// }
