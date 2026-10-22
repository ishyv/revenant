struct Camera { view: vec4f, options: vec4f }
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
struct Vertex {
  @builtin(position) position: vec4f,
  @location(0) uv: vec2f,
  @location(1) color: vec4f,
  @location(2) @interpolate(flat) shape: f32,
  @location(3) atlas_uv: vec2f,
}
@vertex fn vs(@builtin(vertex_index) vertex: u32,
 @location(0) start: vec4f, @location(1) destination: vec4f,
 @location(2) color: vec4f, @location(3) params: vec4f,
 @location(4) tex_rect: vec4f) -> Vertex {
 let corners = array<vec2f,6>(vec2f(0,0),vec2f(1,0),vec2f(0,1),vec2f(0,1),vec2f(1,0),vec2f(1,1));
 let uv = corners[vertex];
 var t = 1.0;
 if params.z > 0.0 {t = clamp((camera.options.x-params.y)/params.z,0.0,1.0);t=t*t*(3.0-2.0*t);}
 let bounds = mix(start,destination,t);
 let world = bounds.xy + uv*bounds.zw;
 let ndc = (world-camera.view.xy)/camera.view.zw;
 var out: Vertex;
 out.position = vec4f(ndc.x*2.0-1.0,1.0-ndc.y*2.0,0,1);
 out.uv=uv; out.color=color; out.shape=params.x;
 out.atlas_uv=tex_rect.xy+uv*tex_rect.zw;
 return out;
}
@fragment fn fs(in: Vertex) -> @location(0) vec4f {
 if in.shape > 1.5 {
   let coverage=textureSample(atlas,atlas_sampler,in.atlas_uv).r;
   return vec4f(in.color.rgb,in.color.a*coverage);
 }
 var alpha=1.0;
 if in.shape > 0.5 {
   let d=length(in.uv*2.0-1.0);
   alpha=1.0-smoothstep(1.0-fwidth(d),1.0,d);
 } else {
   let edge=min(min(in.uv.x,1.0-in.uv.x),min(in.uv.y,1.0-in.uv.y));
   alpha=smoothstep(0.0,fwidth(edge),edge);
 }
 return vec4f(in.color.rgb,in.color.a*alpha);
}
