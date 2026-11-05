/** Shared vertices form a continuous surface; hex outlines are a separate pass. */
export const VERT = `#version 300 es
precision highp float;
layout(location=14)in vec3 aEcosystemColor;layout(location=15)in vec4 aEcosystemPaint;uniform bool uHasEcosystems;uniform highp int uLayer;
layout(location=12)in vec4 aCryosphere;layout(location=13)in vec2 aFrozen;out vec4 vCryosphere;out vec2 vFrozen;
layout(location=10)in float aLake;out float vLake;out vec3 vMapPosition;
layout(location=8)in float aGeography;out float vGeography;
layout(location=9)in float aPartition;out vec3 vPartitionColor;out float vPartitionPresent;
layout(location=0)in vec3 aCenter;layout(location=1)in float aHeight;layout(location=2)in float aBiome;layout(location=3)in float aPlate;layout(location=4)in float aTemperature;layout(location=5)in float aPrecipitation;layout(location=6)in float aFlow;layout(location=7)in vec3 aAtlasPosition;
uniform mat4 uViewProj;uniform float uRadius,uHeightScale;uniform bool uPlanarMode,uWorldFlatMode,uPreviewMode;uniform sampler2D uFrameTex,uFlowTex;uniform float uPreviewMinElev,uPreviewMaxElev,uPreviewHalfHeight;
out vec3 vNormal,vPosition,vBiomeColor;out vec3 vPlateColor;out float vHeight,vBiome,vTemp,vPrecip,vFlow;
out vec2 vAtlasPosition;
out vec3 vPaintColor;out vec4 vVegetation;out vec4 vEcosystemSurface;
layout(location=11)in vec3 aHeightGradient;out vec3 vHeightGradient;
vec3 biome(float value){int b=int(value+.5);
if(b==1)return vec3(.91,.93,.90);if(b==2)return vec3(.65,.69,.58);if(b==3)return vec3(.30,.43,.32);if(b==4)return vec3(.40,.54,.34);if(b==5)return vec3(.66,.71,.43);if(b==6)return vec3(.65,.67,.44);if(b==7)return vec3(.81,.71,.50);if(b==8)return vec3(.73,.71,.45);if(b==9)return vec3(.25,.46,.32);if(b==10)return vec3(.37,.55,.34);if(b==11)return vec3(.67,.69,.55);if(b==12)return vec3(.36,.52,.44);if(b==13)return vec3(.20,.55,.63);return vec3(.55,.64,.56);}
// Separate art palette: the scientific biome layer retains its own colors.
vec3 paintedBiome(float value){int b=int(value+.5);
if(b==1)return vec3(.92,.95,.92);if(b==2)return vec3(.64,.66,.46);if(b==3)return vec3(.23,.40,.28);if(b==4)return vec3(.34,.48,.25);if(b==5)return vec3(.72,.73,.35);if(b==6)return vec3(.66,.66,.34);if(b==7)return vec3(.84,.71,.43);if(b==8)return vec3(.76,.70,.34);if(b==9)return vec3(.18,.38,.25);if(b==10)return vec3(.33,.49,.25);if(b==11)return vec3(.64,.61,.46);if(b==12)return vec3(.34,.52,.39);if(b==13)return vec3(.45,.53,.32);return vec3(.62,.69,.42);}

void main(){
  vCryosphere=uPreviewMode?vec4(0.):aCryosphere;vFrozen=uPreviewMode?vec2(0.):aFrozen;
  vLake=aLake;
  vHeightGradient=aHeightGradient;
  float lon=aAtlasPosition.x*1.57079632679,lat=aAtlasPosition.y*1.57079632679;
  vMapPosition=uWorldFlatMode?vec3(cos(lat)*cos(lon),sin(lat),cos(lat)*sin(lon)):(uPlanarMode?aCenter:normalize(aCenter));
  vGeography=aGeography;
  vPartitionColor=.64+.18*cos(vec3(0.,2.094,4.189)+aPartition*2.399963);vPartitionPresent=step(.5,aPartition);
  vPlateColor=.58+.22*cos(vec3(0.,2.094,4.189)+floor(aPlate+.5)*2.399963);
  vHeight=aHeight;vBiome=aBiome;vTemp=aTemperature;vPrecip=aPrecipitation;vFlow=aFlow;
  vec3 n=uPlanarMode?vec3(0.,0.,1.):normalize(aCenter);
  vec3 pos=uPlanarMode?aCenter:n*(uRadius+max(aHeight,0.)*uHeightScale);
  if(uPreviewMode){
    vec2 uv=uPlanarMode?(uWorldFlatMode?vec2((aAtlasPosition.x+2.)*.25,(1.-aAtlasPosition.y)*.5):vec2((aCenter.x+1.)*.5,(1.-aCenter.y/uPreviewHalfHeight)*.5)):vec2(atan(n.z,n.x)/6.2831853+.5,acos(clamp(n.y,-1.,1.))/3.14159265);
    vec4 s=texture(uFrameTex,uv);
    vHeight=mix(uPreviewMinElev,uPreviewMaxElev,(s.r+256.*s.g)/257.);vBiome=-1.;vTemp=s.b*80.-40.;vPrecip=exp(s.a*8.3)-1.;vFlow=exp(texture(uFlowTex,uv).r*25.);
    vLake=0.;
    if(!uPlanarMode)pos=n*(uRadius+max(vHeight,0.)*uHeightScale);
  }
  vAtlasPosition=aAtlasPosition.xy;
  int b=int(aBiome+.5);
  vVegetation=uPreviewMode?vec4(0.):vec4((b==3||b==4||b==9||b==10)?1.:0.,b==1?1.:0.,b==3?1.:0.,b==6?.8:0.);
  vEcosystemSurface=vec4(0.);
  vPaintColor=uPreviewMode?mix(vec3(.80,.70,.40),vec3(.35,.53,.30),clamp(vPrecip/1800.,0.,1.)):paintedBiome(aBiome);
  if(uHasEcosystems&&!uPreviewMode){
    vPaintColor=aEcosystemPaint.rgb;
    int material=int(aEcosystemPaint.w+.5);
    // Decode categorical IDs before interpolation across ecotones and seams.
    vVegetation=vec4(0.);vEcosystemSurface=vec4(0.);
    if(material>=1&&material<=7){vVegetation.x=material==2?.62:1.;vVegetation.z=material==5?1.:(material==6?.45:0.);}
    if(material==1)vVegetation.w=.08;
    if(material==2){vVegetation.w=.18;vEcosystemSurface.x=.25;}
    if(material==3){vVegetation.z=.15;vEcosystemSurface.y=.35;}
    if(material==4){vVegetation.x=.82;vVegetation.w=.12;vEcosystemSurface.x=.2;}
    if(material==6){vVegetation.x=.9;vVegetation.w=.15;}
    if(material==7){vVegetation.x=.65;vVegetation.w=.28;vEcosystemSurface.x=.3;}
    if(material==8||material==9)vVegetation.w=.8;
    if(material==10||material==11||material==13||material==17)vEcosystemSurface.x=1.;
    if(material==11){vVegetation.x=.18;vVegetation.w=.2;}
    if(material==15)vVegetation.y=1.;
    if(material==16)vEcosystemSurface.z=1.;
    if(material==18){vVegetation.w=.2;vEcosystemSurface.x=.5;}
    if(material==19){vVegetation.x=1.;vEcosystemSurface.y=.8;}
    if(material==20||material==21){vVegetation.w=.25;vEcosystemSurface.y=1.;}
    if(material==12||material==14)vEcosystemSurface.w=1.;
  }
  vNormal=n;vPosition=uPlanarMode?aCenter:n;vBiomeColor=uPreviewMode?mix(vec3(.79,.72,.52),vec3(.42,.59,.39),clamp(vPrecip/1800.,0.,1.)):(uHasEcosystems&&uLayer==3?aEcosystemColor:biome(aBiome));
  gl_Position=uViewProj*vec4(pos,1.);
}`
export const FRAG = `#version 300 es
precision highp float;
in vec4 vCryosphere;in vec2 vFrozen;uniform bool uHasCryosphere,uHasEcosystems;
in float vLake;in vec3 vMapPosition;
in float vGeography;uniform bool uGeographyActive;
in vec3 vPartitionColor;in float vPartitionPresent;
in vec3 vNormal,vPosition,vBiomeColor;in vec3 vPlateColor;in float vHeight,vBiome,vTemp,vPrecip,vFlow;
in vec2 vAtlasPosition;
in vec3 vPaintColor;in vec4 vVegetation;in vec4 vEcosystemSurface;
in vec3 vHeightGradient;uniform bool uPreviewMode;
uniform sampler2D uPigmentTex;
uniform bool uPlanarMode,uWorldFlatMode;uniform float uMapMetres;uniform vec3 uLightDir;uniform highp int uStyle,uLayer,uProjection;out vec4 fragColor;
vec3 biome(float value){int b=int(value+.5);
if(b==1)return vec3(.91,.93,.90);if(b==2)return vec3(.65,.69,.58);if(b==3)return vec3(.30,.43,.32);if(b==4)return vec3(.40,.54,.34);if(b==5)return vec3(.66,.71,.43);if(b==6)return vec3(.65,.67,.44);if(b==7)return vec3(.81,.71,.50);if(b==8)return vec3(.73,.71,.45);if(b==9)return vec3(.25,.46,.32);if(b==10)return vec3(.37,.55,.34);if(b==11)return vec3(.67,.69,.55);if(b==12)return vec3(.36,.52,.44);if(b==13)return vec3(.20,.55,.63);return vec3(.55,.64,.56);}
vec3 ocean(float h){return mix(vec3(.60,.73,.72),vec3(.30,.48,.53),clamp(-h/6500.,0.,1.));}
vec3 temperature(float t){float n=clamp((t+40.)/80.,0.,1.);return n<.5?mix(vec3(.35,.48,.70),vec3(.78,.82,.64),n*2.):mix(vec3(.78,.82,.64),vec3(.77,.35,.24),(n-.5)*2.);}
vec3 precipitation(float p){return mix(vec3(.83,.74,.51),vec3(.21,.45,.58),pow(clamp(p/3000.,0.,1.),.6));}
vec3 elevation(float e){if(e<0.)return ocean(e);if(e<500.)return mix(vec3(.55,.68,.45),vec3(.74,.74,.50),e/500.);if(e<2500.)return mix(vec3(.74,.74,.50),vec3(.62,.53,.42),(e-500.)/2000.);return mix(vec3(.62,.53,.42),vec3(.94,.94,.89),clamp((e-2500.)/4000.,0.,1.));}
float terrainHash(vec3 p){p=fract(p*.1031);p+=dot(p,p.yzx+33.33);return fract((p.x+p.y)*p.z);}
float terrainGrain(vec3 p){
  vec3 i=floor(p),f=fract(p);f=f*f*(3.-2.*f);
  return mix(mix(mix(terrainHash(i),terrainHash(i+vec3(1,0,0)),f.x),mix(terrainHash(i+vec3(0,1,0)),terrainHash(i+vec3(1,1,0)),f.x),f.y),mix(mix(terrainHash(i+vec3(0,0,1)),terrainHash(i+vec3(1,0,1)),f.x),mix(terrainHash(i+vec3(0,1,1)),terrainHash(i+vec3(1,1,1)),f.x),f.y),f.z);
}
vec3 gameMap(){
  bool plane=uPlanarMode&&!uWorldFlatMode;
  vec3 map=plane?vMapPosition:normalize(vMapPosition);
  // Material coordinates stay on the geography, including across the atlas seam.
  // Mipmaps suppress pigment detail below a pixel; no screen-space grain flicker.
  float materialScale=clamp(uMapMetres/1200000.,.6,6.);
  vec3 p=map*materialScale;
  vec3 weights=plane?vec3(0.,0.,1.):pow(abs(map),vec3(4.));
  weights/=max(dot(weights,vec3(1.)),1.e-5);
  float pigment=dot(vec3(texture(uPigmentTex,p.yz).r,texture(uPigmentTex,p.zx).r,texture(uPigmentTex,p.xy).r),weights);
  float brush=terrainGrain(p*7.);
  float grain=terrainGrain(p*85.);
  float visible=1.-smoothstep(.4,1.5,length(fwidth(p*85.)));
  float wash=(pigment-.552)*.85+(brush-.5)*.055;
  vec3 up=plane?vec3(0.,0.,1.):map;
  float reliefScale=clamp(uMapMetres/85000.,1.4,80.);
  vec3 relief=plane?vec3(map.xy*uMapMetres,vHeight*reliefScale):up*(uMapMetres+vHeight*reliefScale);
  vec3 faceNormal=normalize(cross(dFdx(relief),dFdy(relief)));
  if(dot(faceNormal,up)<0.)faceNormal=-faceNormal;
  vec3 gradient=vHeightGradient/max(uMapMetres,1.);
  gradient-=up*dot(gradient,up);
  vec3 normal=uPreviewMode?faceNormal:normalize(up-gradient*reliefScale);
  vec3 east=plane?vec3(1.,0.,0.):normalize(vec3(-up.z,0.,up.x)+vec3(1.e-6,0.,0.));
  vec3 north=plane?vec3(0.,1.,0.):normalize(cross(east,up));
  vec3 light=normalize(up*.8+north*.65-east*.55);
  vec3 fill=normalize(up+north*.15+east*.6);
  float shade=.43+.62*max(dot(normal,light),0.)+.16*max(dot(normal,fill),0.);
  if(vHeight<=0.){
    float depth=max(-vHeight,0.);
    vec3 sea=mix(vec3(.35,.71,.70),vec3(.12,.43,.51),smoothstep(0.,1800.,depth));
    sea=mix(sea,vec3(.055,.25,.34),smoothstep(1200.,6800.,depth));
    sea=mix(sea,vec3(.67,.82,.72),(1.-smoothstep(0.,80.,depth))*.45);
    sea*=(1.+wash*1.8)*(.94+.06*shade);
    float seaIce=clamp(vCryosphere.y*(.96+.04*grain),0.,1.);
    sea=mix(sea,vec3(.85,.94,.95)*(.96+wash*.25),seaIce);
    return mix(sea,vec3(.93,.96,.95)*(.96+wash*.18),clamp(max(vCryosphere.x,vCryosphere.z),0.,1.));
  }
  vec3 color=vPaintColor;
  float slope=1.-max(dot(normal,up),0.);
  float rock=smoothstep(1800.,4800.,vHeight)*.60+smoothstep(.16,.65,slope)*.22;
  color=mix(color,vec3(.64,.57,.44),min(rock,.78));
  float forest=clamp(vVegetation.x,0.,1.);
  float needle=terrainGrain(p*vec3(115.,55.,115.));
  float crowns=mix(smoothstep(.35,.8,grain),smoothstep(.43,.72,needle),clamp(vVegetation.z,0.,1.));
  color*=1.-forest*crowns*.22*visible;
  color*=1.-vVegetation.w*smoothstep(.48,.72,brush)*.11;
  color+=vec3(.035,.027,-.012)*vEcosystemSurface.x*(grain-.5)*visible;
  color=mix(color,color*vec3(.87,.94,.91),vEcosystemSurface.y*(.25+.35*brush));
  color=mix(color,vec3(.57,.53,.46),vEcosystemSurface.z*.3*smoothstep(.25,.75,grain)*visible);
  color+=vec3(.025,.016,.005)*vEcosystemSurface.w*(brush-.5);
  color*=shade*(1.+wash+(grain-.5)*.04*visible);
  float snow=uHasCryosphere?max(vCryosphere.x,vCryosphere.w):max(clamp(vVegetation.y,0.,1.),smoothstep(4200.,5700.,vHeight)*(1.-smoothstep(-8.,5.,vTemp)));
  color=mix(color,vec3(.93,.96,.94)*(.88+.12*shade+wash*.3),snow);
  if(vLake>.5)color=mix(color,mix(vec3(.20,.55,.63),vec3(.88,.95,.95),uHasCryosphere?clamp(vFrozen.y,0.,1.):1.-smoothstep(-8.,0.,vTemp)),smoothstep(.5,3.,vLake)*(uHasCryosphere?1.-clamp(vCryosphere.x,0.,1.):1.));
  return mix(color,vec3(.96,.93,.82),.018);
}
void main(){if(uWorldFlatMode && (abs(vAtlasPosition.x)>2.00001 || abs(vAtlasPosition.y)>1.00001 || (uProjection==1 && abs(vAtlasPosition.y)>.9450125416934075)))discard;vec3 color;
if(uLayer==10)color=gameMap();else if(uLayer==7)color=vHeight<=0.?mix(ocean(vHeight),vec3(.87,.90,.87),.35):mix(vec3(.89,.88,.82),vPartitionColor,smoothstep(.25,.75,vPartitionPresent));else if(uLayer==6)color=vPlateColor;else if(uLayer==1)color=temperature(vTemp);else if(uLayer==2)color=precipitation(vPrecip);else if(uLayer==3)color=uHasEcosystems&&!uPreviewMode?vBiomeColor:(vHeight<=0.?ocean(vHeight):vBiomeColor);else if(uLayer==4)color=elevation(vHeight);else if(uLayer==5){float flow=clamp((log(max(vFlow,1.))-log(1.e9))/(log(1.e12)-log(1.e9)),0.,1.);color=vHeight<=0.?ocean(vHeight):mix(vec3(.84,.84,.71),vec3(.16,.40,.55),flow);}
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
if(uLayer==11){color=vHeight<=0.?mix(vec3(.19,.40,.53),vec3(.84,.94,.95),clamp(vCryosphere.y,0.,1.)):mix(vec3(.70,.74,.61),vec3(.57,.49,.68),clamp(vFrozen.x,0.,1.));if(vHeight>0.&&vLake>.5)color=mix(color,mix(vec3(.19,.40,.53),vec3(.84,.94,.95),clamp(vFrozen.y,0.,1.)),smoothstep(.5,3.,vLake));color=mix(color,vec3(.97,.98,.96),clamp(vHeight<=0.?max(vCryosphere.x,vCryosphere.z):max(vCryosphere.x,vCryosphere.w),0.,1.));}
if(uLayer==0&&uHasCryosphere&&!uPreviewMode){float ice=vHeight<=0.?max(vCryosphere.x,max(vCryosphere.y,vCryosphere.z)):max(max(vCryosphere.x,vCryosphere.w),vLake>.5?vFrozen.y:0.);color=mix(color,vec3(.91,.96,.95),clamp(ice,0.,1.));}
if(uGeographyActive){float member=smoothstep(.25,.75,vGeography);color=mix(mix(color,vec3(.94,.92,.85),.48),mix(color,vec3(.28,.52,.36),.22),member);}
fragColor=vec4(color,1.);
}`

export const LINE_VERT = `#version 300 es
precision highp float;layout(location=0)in vec3 aPosition;layout(location=1)in vec3 aAtlasPosition;
uniform mat4 uViewProj;out vec3 vPosition;out vec2 vAtlasPosition;
void main(){vPosition=aPosition;vAtlasPosition=aAtlasPosition.xy;gl_Position=uViewProj*vec4(aPosition,1.);}`
export const LINE_FRAG = `#version 300 es
precision highp float;in vec3 vPosition;in vec2 vAtlasPosition;uniform bool uSphere,uWorldFlatMode;uniform int uProjection;uniform vec3 uEye;uniform float uRadius;uniform vec4 uColor;out vec4 fragColor;
void main(){if(uSphere && dot(normalize(vPosition),uEye)<uRadius)discard;if(uWorldFlatMode && (abs(vAtlasPosition.x)>2.00001||abs(vAtlasPosition.y)>1.00001||(uProjection==1 && abs(vAtlasPosition.y)>.9450125416934075)))discard;fragColor=uColor;}`
