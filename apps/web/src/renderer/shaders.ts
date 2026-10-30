/** Shared vertices form a continuous surface; hex outlines are a separate pass. */
export const VERT = `#version 300 es
precision highp float;
layout(location=0)in vec3 aCenter;layout(location=1)in float aHeight;layout(location=2)in float aBiome;layout(location=4)in float aTemperature;layout(location=5)in float aPrecipitation;layout(location=6)in float aFlow;
uniform mat4 uViewProj;uniform float uRadius,uHeightScale;uniform bool uPlanarMode,uWorldFlatMode,uPreviewMode;uniform sampler2D uFrameTex;uniform float uPreviewMinElev,uPreviewMaxElev;
out vec3 vNormal,vPosition,vBiomeColor;out float vHeight,vBiome,vTemp,vPrecip,vFlow;
vec3 biome(float value){int b=int(value+.5);
if(b==1)return vec3(.91,.93,.90);if(b==2)return vec3(.65,.69,.58);if(b==3)return vec3(.30,.43,.32);if(b==4)return vec3(.40,.54,.34);if(b==5)return vec3(.66,.71,.43);if(b==6)return vec3(.65,.67,.44);if(b==7)return vec3(.81,.71,.50);if(b==8)return vec3(.73,.71,.45);if(b==9)return vec3(.25,.46,.32);if(b==10)return vec3(.37,.55,.34);if(b==11)return vec3(.67,.69,.55);if(b==12)return vec3(.36,.52,.44);return vec3(.55,.64,.56);}

void main(){
  vHeight=aHeight;vBiome=aBiome;vTemp=aTemperature;vPrecip=aPrecipitation;vFlow=aFlow;
  vec3 n=uPlanarMode?vec3(0.,0.,1.):normalize(aCenter);
  vec3 pos=uPlanarMode?aCenter:n*(uRadius+max(aHeight,0.)*uHeightScale);
  if(uPreviewMode){
    vec2 uv=uPlanarMode?vec2(uWorldFlatMode?(aCenter.x+2.)*.25:(aCenter.x+1.)*.5,(1.-aCenter.y)*.5):vec2(atan(n.z,n.x)/6.2831853+.5,acos(clamp(n.y,-1.,1.))/3.14159265);
    vec4 s=texture(uFrameTex,uv);
    vHeight=mix(uPreviewMinElev,uPreviewMaxElev,s.r);vBiome=-1.;vTemp=s.g*80.-40.;vPrecip=exp(s.b*8.3)-1.;vFlow=exp(s.a*25.);
  }
  vNormal=n;vPosition=uPlanarMode?aCenter:n;vBiomeColor=uPreviewMode?mix(vec3(.79,.72,.52),vec3(.42,.59,.39),clamp(vPrecip/1800.,0.,1.)):biome(aBiome);
  gl_Position=uViewProj*vec4(pos,1.);
}`
export const FRAG = `#version 300 es
precision highp float;
in vec3 vNormal,vPosition,vBiomeColor;in float vHeight,vBiome,vTemp,vPrecip,vFlow;
uniform bool uPlanarMode,uWorldFlatMode;uniform float uMapMetres;uniform vec3 uLightDir;uniform int uStyle,uLayer;out vec4 fragColor;
vec3 biome(float value){int b=int(value+.5);
if(b==1)return vec3(.91,.93,.90);if(b==2)return vec3(.65,.69,.58);if(b==3)return vec3(.30,.43,.32);if(b==4)return vec3(.40,.54,.34);if(b==5)return vec3(.66,.71,.43);if(b==6)return vec3(.65,.67,.44);if(b==7)return vec3(.81,.71,.50);if(b==8)return vec3(.73,.71,.45);if(b==9)return vec3(.25,.46,.32);if(b==10)return vec3(.37,.55,.34);if(b==11)return vec3(.67,.69,.55);if(b==12)return vec3(.36,.52,.44);return vec3(.55,.64,.56);}
vec3 ocean(float h){return mix(vec3(.60,.73,.72),vec3(.30,.48,.53),clamp(-h/6500.,0.,1.));}
vec3 temperature(float t){float n=clamp((t+40.)/80.,0.,1.);return n<.5?mix(vec3(.35,.48,.70),vec3(.78,.82,.64),n*2.):mix(vec3(.78,.82,.64),vec3(.77,.35,.24),(n-.5)*2.);}
vec3 precipitation(float p){return mix(vec3(.83,.74,.51),vec3(.21,.45,.58),pow(clamp(p/3000.,0.,1.),.6));}
vec3 elevation(float e){if(e<0.)return ocean(e);if(e<500.)return mix(vec3(.55,.68,.45),vec3(.74,.74,.50),e/500.);if(e<2500.)return mix(vec3(.74,.74,.50),vec3(.62,.53,.42),(e-500.)/2000.);return mix(vec3(.62,.53,.42),vec3(.94,.94,.89),clamp((e-2500.)/4000.,0.,1.));}
void main(){if(uWorldFlatMode && (abs(vPosition.x)>2.00001 || abs(vPosition.y)>1.00001))discard;vec3 color;
if(uLayer==1)color=temperature(vTemp);else if(uLayer==2)color=precipitation(vPrecip);else if(uLayer==3)color=vHeight<=0.?ocean(vHeight):vBiomeColor;else if(uLayer==4)color=elevation(vHeight);else if(uLayer==5){float flow=clamp((log(max(vFlow,1.))-log(1.e9))/(log(1.e12)-log(1.e9)),0.,1.);color=vHeight<=0.?ocean(vHeight):mix(vec3(.84,.84,.71),vec3(.16,.40,.55),flow);}
else{
  color=vHeight<=0.?ocean(vHeight):vBiomeColor;
  if(vBiome<-.5&&vHeight>0.)color=mix(vec3(.79,.72,.52),vec3(.42,.59,.39),clamp(vPrecip/1800.,0.,1.));
  if(vHeight>1500.)color=mix(color,vec3(.66,.60,.51),clamp((vHeight-1500.)/4000.,0.,.7));
  vec3 reliefPosition=uPlanarMode?vec3(vPosition.xy,vHeight/max(uMapMetres,1.)*8.):normalize(vPosition)*(1.+vHeight/max(uMapMetres,1.)*8.);
  vec3 reliefNormal=normalize(cross(dFdx(reliefPosition),dFdy(reliefPosition)));
  float shade=.73+.27*max(dot(reliefNormal,normalize(uLightDir)),0.);color*=shade;
  if(uStyle==1)color=mix(vec3(dot(color,vec3(.299,.587,.114))),color,.68)*1.07;
  if(uStyle==2)color=mix(color,vec3(.94,.90,.79),.16);
  if(uStyle==3&&vHeight>0.){color=elevation(vHeight);float contour=abs(fract(vHeight/200.-.5)-.5)/max(fwidth(vHeight/200.),1.e-5);color=mix(color,vec3(.32,.32,.23),(1.-min(contour,1.))*.55);}
}
fragColor=vec4(color,1.);
}`

export const LINE_VERT = `#version 300 es
precision highp float;layout(location=0)in vec3 aPosition;
uniform mat4 uViewProj;out vec3 vPosition;
void main(){vPosition=aPosition;gl_Position=uViewProj*vec4(aPosition,1.);}`
export const LINE_FRAG = `#version 300 es
precision highp float;in vec3 vPosition;uniform bool uSphere,uWorldFlatMode;uniform vec3 uEye;uniform float uRadius;out vec4 fragColor;
void main(){if(uSphere && dot(normalize(vPosition),uEye)<uRadius)discard;if(uWorldFlatMode && (abs(vPosition.x)>2.00001||abs(vPosition.y)>1.00001))discard;fragColor=vec4(.27,.36,.29,.22);}`
