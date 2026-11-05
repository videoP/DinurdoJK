// Per-source EAGLE/EAX scene. EalMan and OpenAL are confined to one worker.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include "eal_scene.h"
#include "EalMan.h"
#include "OpenAL/al.h"
#include "OpenAL/alc.h"
#include "OpenAL/alext.h"
#define OPENAL
#include "eax/eax.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <cctype>
#include <cstring>
#include <map>
#include <memory>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

static_assert(sizeof(DjkEalVoice)==1144);
static_assert(sizeof(DjkEalFrame)==109872);
static_assert(sizeof(DjkEalStats)==72);

namespace {
const GUID slots[4] = {
 {0xc4d79f1e,0xf1ac,0x436b,{0xa8,0x1d,0xa7,0x38,0xe7,0x04,0x54,0x69}},
 {0x08c00e96,0x74be,0x4491,{0x93,0xaa,0xe8,0xad,0x35,0xa4,0x91,0x17}},
 {0x1d433b88,0xf0f6,0x4637,{0x91,0x9f,0x60,0xe7,0xe0,0x6b,0x5e,0xdd}},
 {0xefff08ea,0xc7d8,0x44ab,{0x93,0xad,0x6d,0xbd,0x5f,0x91,0x00,0x64}}};
const GUID context_id={0x1d4870ad,0x0def,0x43c0,{0xa4,0x0c,0x52,0x36,0x32,0x29,0x63,0x42}};
const GUID source_id={0x1b86b823,0x22df,0x4eae,{0x8b,0x3c,0x12,0x78,0xce,0x54,0x42,0x27}};
const GUID reverb_id={0xcf95c8f,0xa3cc,0x4849,{0xb0,0xb6,0x83,0x2e,0xcc,0x18,0x22,0xdf}};
const GUID primary_id={0xf317866d,0x924c,0x450c,{0x86,0x1b,0xe6,0xda,0xa2,0x5e,0x7c,0x20}};
const GUID null_id={};

EMPoint eagle(const float* p) { return {p[0],p[2],p[1]}; }
std::array<float,3> openal(const float* p) { return {p[0],p[2],-p[1]}; }
float distance(EMPoint a, EMPoint b) { return std::sqrt((a.fX-b.fX)*(a.fX-b.fX)+(a.fY-b.fY)*(a.fY-b.fY)+(a.fZ-b.fZ)*(a.fZ-b.fZ)); }
float length(const std::array<float,3>& a) { return std::sqrt(a[0]*a[0]+a[1]*a[1]+a[2]*a[2]); }
void error_text(char* out, uint32_t cap, const char* text) {
    if(cap) { std::strncpy(out,text,cap-1); out[cap-1]=0; }
}

struct Aperture { EMPoint a,b,center; };
// The ALC_SOFT_loopback renderer is synchronous. Source PCM callbacks execute
// on this same native worker during alcRenderSamplesSOFT (no audio callback
// thread, mutex, or allocation). AL_SOFT_callback_buffer avoids the repeated
// short AL streaming queue drain/restart seen in the diagnostic tone test.
constexpr size_t CALLBACK_FIFO = size_t(DJK_EAL_FRAMES) * 8;
struct CallbackTelemetry {
    uint64_t starves=0;
    uint64_t underfeed_samples=0;
    uint64_t dropped_samples=0;
};
struct Source {
    ALuint source=0;
    std::array<ALuint,3> buffers{};
    uint64_t id=0;
    uint32_t next=0;
    int queued=0;
    EAXSOURCEPROPERTIES properties{};
    bool properties_valid=false;
    EAXSOURCEALLSENDPROPERTIES send{};
    bool send_valid=false;
    std::array<bool,3> buffer_queued{};
    bool ever_started=false;
    bool relative_mode=false;
    bool callback_mode=false;
    bool callback_draining=false;
    size_t callback_tail=0;
    bool callback_tone=false;
    uint64_t callback_tone_sample=0;
    uint32_t callback_rate=48000;
    std::array<float,CALLBACK_FIFO> callback_fifo{};
    size_t callback_read=0;
    size_t callback_used=0;
    CallbackTelemetry* telemetry=nullptr;
    bool pcm_previous_valid=false;
    float last_pcm=0.0f;
    uint64_t tone_sample_index=0;
    EAXACTIVEFXSLOTS active_sends{};
    bool active_sends_valid=false;
    SourceAttributes model{};
    EAXSOURCEPROPERTIES base{};
};

// Called only while Scene::process is synchronously rendering its loopback
// block. Full-sized reads keep the AL source alive through brief FIFO gaps.
ALsizei AL_APIENTRY source_pcm_callback(ALvoid* user, ALvoid* samples, ALsizei bytes) noexcept {
    auto& s=*static_cast<Source*>(user);
    if(bytes<=0) return 0;
    const auto wanted=size_t(bytes)/sizeof(float);
    auto* dst=static_cast<float*>(samples);
    if(s.callback_tone && !s.callback_draining) {
        constexpr double pi=3.14159265358979323846;
        for(size_t i=0;i<wanted;++i)
            dst[i]=0.025f*float(std::sin(2.0*pi*440.0*double(s.callback_tone_sample++)/double(s.callback_rate)));
        return bytes;
    }
    size_t copied=0;
    while(copied<wanted && s.callback_used) {
        const auto chunk=std::min({wanted-copied,s.callback_used,CALLBACK_FIFO-s.callback_read});
        std::memcpy(dst+copied,s.callback_fifo.data()+s.callback_read,chunk*sizeof(float));
        s.callback_read=(s.callback_read+chunk)%CALLBACK_FIFO;
        s.callback_used-=chunk;
        copied+=chunk;
    }
    if(s.callback_draining) {
        // OpenAL extends the last returned sample across its resampler read
        // window at EOF. Finish with actual silence, so an exact-block sound
        // ending on a nonzero sample is not stretched into an audible plateau.
        const auto zeros=std::min(wanted-copied,s.callback_tail);
        std::fill(dst+copied,dst+copied+zeros,0.0f);
        s.callback_tail-=zeros;
        return ALsizei((copied+zeros)*sizeof(float));
    }
    if(copied<wanted) {
        std::fill(dst+copied,dst+wanted,0.0f);
        ++s.telemetry->starves;
        s.telemetry->underfeed_samples+=wanted-copied;
    }
    return ALsizei(wanted*sizeof(float));
}
void callback_append(Source& s,const float* pcm) {
    if(CALLBACK_FIFO-s.callback_used < DJK_EAL_FRAMES) {
        s.telemetry->dropped_samples+=DJK_EAL_FRAMES;
        return; // Never overwrite data the OpenAL callback has not consumed.
    }
    size_t tail=(s.callback_read+s.callback_used)%CALLBACK_FIFO;
    for(size_t i=0;i<DJK_EAL_FRAMES;++i) {
        s.callback_fifo[tail]=pcm[i];
        tail=(tail+1)%CALLBACK_FIFO;
    }
    s.callback_used+=DJK_EAL_FRAMES;
}
struct Scene {
    HMODULE module=nullptr;
    ALCdevice* device=nullptr;
    ALCcontext* context=nullptr;
    uint32_t rate=0;
    int requested_hrtf=-1;
    int hrtf=0;
    ALCint hrtf_status=ALC_HRTF_DISABLED_SOFT;
    CallbackTelemetry telemetry;
    EalMan manager;
    std::array<Source,DJK_EAL_VOICES> sources{};
    std::array<int,4> environments{{-999,-998,-999,-999}};
    std::map<int,std::vector<Aperture>> apertures;
    SourceAttributes default_model{};
    EAXSOURCEPROPERTIES default_properties{};
    std::map<std::string,int32_t> file_models;
    std::array<bool,4> room_active{};
    int listener_environment=-1;
    // Lifetime counters reset on a debug-mode switch (not every render block).
    uint32_t queue_starves=0;
    uint32_t unexpected_restarts=0;
    uint32_t multiple_processed=0;
    uint32_t buffer_reuse=0;
    float max_pcm_edge=0.0f;
    float max_output_edge=0.0f;
    std::array<float,2> previous_output{};
    bool previous_output_valid=false;
    uint32_t current_debug_mode=0;
    uint32_t last_debug_mode=UINT32_MAX;
    int last_primary=-1;
    std::array<int32_t,4> last_volume{{INT32_MIN,INT32_MIN,INT32_MIN,INT32_MIN}};
    std::array<EAXVECTOR,4> last_pan{};
    std::array<bool,4> last_pan_valid{};
    // Dynamic loading keeps native EAL optional; absence preserves legacy audio.
#define AL_FN(name) decltype(&name) p_##name=nullptr
    AL_FN(alGetError); AL_FN(alIsExtensionPresent); AL_FN(alGetProcAddress);
    AL_FN(alGenSources); AL_FN(alDeleteSources); AL_FN(alGenBuffers); AL_FN(alDeleteBuffers);
    AL_FN(alSourcei); AL_FN(alSourcef); AL_FN(alSource3f); AL_FN(alSourcePlay); AL_FN(alSourceStop);
    AL_FN(alGetSourcei); AL_FN(alSourceQueueBuffers); AL_FN(alSourceUnqueueBuffers); AL_FN(alBufferData);
    AL_FN(alListenerfv); AL_FN(alDistanceModel); AL_FN(alDopplerFactor);
    AL_FN(alcGetProcAddress); AL_FN(alcCreateContext); AL_FN(alcDestroyContext); AL_FN(alcCloseDevice); AL_FN(alcGetError); AL_FN(alcGetIntegerv);
#undef AL_FN
    LPALCLOOPBACKOPENDEVICESOFT loop_open=nullptr;
    LPALCRENDERSAMPLESSOFT render=nullptr;
    LPALBUFFERCALLBACKSOFT callback_buffer=nullptr;
    PFNALCSETTHREADCONTEXTPROC make=nullptr;
    LPALCRESETDEVICESOFT reset=nullptr;
    EAXSet set=nullptr;
    EAXGet get=nullptr;

    template<class T> T symbol(const char* name) {
        auto value=GetProcAddress(module,name);
        if(!value) throw std::runtime_error(std::string("OpenAL missing ")+name);
        return reinterpret_cast<T>(value);
    }
    template<class T> T extension(const char* name) {
        auto value=p_alcGetProcAddress(device,name);
        if(!value) throw std::runtime_error(std::string("OpenAL missing ")+name);
        return reinterpret_cast<T>(value);
    }
    template<class T> void eax(const GUID& guid, ALuint property, ALuint source, const T& value) {
        auto result=set(&guid,property,source,const_cast<T*>(&value),sizeof(value));
        if(result!=AL_NO_ERROR) throw std::runtime_error("OpenAL EAX property "+std::to_string(property)+" failed: "+std::to_string(result));
    }
    void check(const char* operation) {
        auto al=p_alGetError(); auto alc=p_alcGetError(device);
        if(al || alc) throw std::runtime_error(std::string(operation)+": AL "+std::to_string(al)+", ALC "+std::to_string(alc));
    }
    ~Scene() {
        if(context && make) {
            make(context);
            for(auto& s:sources) if(s.source) {
                p_alSourceStop(s.source); p_alDeleteSources(1,&s.source);
                p_alDeleteBuffers(3,s.buffers.data());
            }
            make(nullptr); p_alcDestroyContext(context);
        }
        if(device && p_alcCloseDevice) p_alcCloseDevice(device);
        if(module) FreeLibrary(module);
    }
    void initialize(const uint16_t* path,const uint8_t* bytes,uint32_t size,uint32_t sample_rate) {
        rate=sample_rate;
        module=LoadLibraryExW(reinterpret_cast<const wchar_t*>(path),nullptr,LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR|LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        if(!module) throw std::runtime_error("OpenALSoft.x86_64.dll or its Microsoft runtime dependencies are unavailable");
#define LOAD(name) p_##name=symbol<decltype(p_##name)>(#name)
        LOAD(alGetError);LOAD(alIsExtensionPresent);LOAD(alGetProcAddress);
        LOAD(alGenSources);LOAD(alDeleteSources);LOAD(alGenBuffers);LOAD(alDeleteBuffers);
        LOAD(alSourcei);LOAD(alSourcef);LOAD(alSource3f);LOAD(alSourcePlay);LOAD(alSourceStop);
        LOAD(alGetSourcei);LOAD(alSourceQueueBuffers);LOAD(alSourceUnqueueBuffers);LOAD(alBufferData);
        LOAD(alListenerfv);LOAD(alDistanceModel);LOAD(alDopplerFactor);
        LOAD(alcGetProcAddress);LOAD(alcCreateContext);LOAD(alcDestroyContext);LOAD(alcCloseDevice);LOAD(alcGetError);LOAD(alcGetIntegerv);
#undef LOAD
        loop_open=extension<LPALCLOOPBACKOPENDEVICESOFT>("alcLoopbackOpenDeviceSOFT");
        device=loop_open(nullptr);
        if(!device) throw std::runtime_error("OpenAL loopback device unavailable");
        make=extension<PFNALCSETTHREADCONTEXTPROC>("alcSetThreadContext");
        render=extension<LPALCRENDERSAMPLESSOFT>("alcRenderSamplesSOFT");
        reset=extension<LPALCRESETDEVICESOFT>("alcResetDeviceSOFT");
        const ALCint attributes[]={ALC_FORMAT_CHANNELS_SOFT,ALC_STEREO_SOFT,ALC_FORMAT_TYPE_SOFT,ALC_FLOAT_SOFT,
            ALC_FREQUENCY,static_cast<ALCint>(rate),ALC_MAX_AUXILIARY_SENDS,4,ALC_HRTF_SOFT,ALC_FALSE,0};
        context=p_alcCreateContext(device,attributes);
        if(!context || !make(context)) throw std::runtime_error("OpenAL EAL context unavailable");
        if(!p_alIsExtensionPresent("EAX4.0")) throw std::runtime_error("OpenAL runtime lacks EAX 4.0");
        get=reinterpret_cast<EAXGet>(p_alGetProcAddress("EAXGet"));
        set=reinterpret_cast<EAXSet>(p_alGetProcAddress("EAXSet"));
        if(p_alIsExtensionPresent("AL_SOFT_callback_buffer"))
            callback_buffer=reinterpret_cast<LPALBUFFERCALLBACKSOFT>(p_alGetProcAddress("alBufferCallbackSOFT"));
        if(!set || !get) throw std::runtime_error("OpenAL missing EAXSet");
        std::istringstream data(std::string(reinterpret_cast<const char*>(bytes),size),std::ios::binary);
        if(manager.LoadDataSetFromMemory(data)!=0) throw std::runtime_error("EalMan rejected the EAL dataset (JKA EAL 3.0 required)");
        manager.GetSourceAttributes(-1,default_model);
        // dsrc is the editor template for authored models, not an instruction
        // to replace the runtime defaults of every game-created source.
        ALuint probe=0; p_alGenSources(1,&probe);
        if(get(&source_id,EAXSOURCE_ALLPARAMETERS,probe,&default_properties,sizeof(default_properties))!=AL_NO_ERROR)
            throw std::runtime_error("OpenAL EAX default source query failed");
        p_alDeleteSources(1,&probe);
        read_file_models(bytes,size);
        p_alDistanceModel(AL_NONE); // JKA distance/channel gain is in the PCM send.
        p_alDopplerFactor(0.0f); // Demo/source pitch is already in the PCM clock.
        EAXFXSLOTPROPERTIES slot{reverb_id,0,EAXFXSLOT_LOCKED,EAXFXSLOTFLAGS_ENVIRONMENT};
        // EAX4 reserves slot 0 for reverb and slot 1 for chorus. Soft rejects
        // reloading those legacy slots; rooms use the existing slot 0 plus 2/3.
        for(int i=2;i<4;++i) eax(slots[i],EAXFXSLOT_ALLPARAMETERS,0,slot);
        eax(slots[0],EAXFXSLOT_FLAGS,0,uint32_t(EAXFXSLOTFLAGS_ENVIRONMENT));
        read_apertures();
        check("EAL scene initialization");
    }
    static std::string canonical(std::string name) {
        std::replace(name.begin(),name.end(),'\\','/');
        std::transform(name.begin(),name.end(),name.begin(),[](unsigned char c){return char(std::tolower(c));});
        return name;
    }
    void read_file_models(const uint8_t* bytes,uint32_t size) {
        // EalMan exposes model names but not the associated FILS paths.
        // Read only bounded source-list paths; geometry remains EalMan-owned.
        auto word=[&](size_t i) { uint32_t n=0;std::memcpy(&n,bytes+i,4);return n; };
        for(size_t at=12;at+8<=size;) {
            size_t end=at+8+word(at+4); if(end>size) break;
            if(end>=at+12 && !std::memcmp(bytes+at,"LIST",4) && !std::memcmp(bytes+at+8,"srcp",4)) {
                uint32_t count=0;
                for(size_t p=at+12;p+8<=end;) {
                    size_t n=word(p+4),next=p+8+n; if(next>end) break;
                    if(!std::memcmp(bytes+p,"num ",4) && n==4) count=word(p+8);
                    if(!std::memcmp(bytes+p,"fils",4) && count<=n/260) for(uint32_t i=0;i<count;++i) {
                        const char* text=reinterpret_cast<const char*>(bytes+p+8+i*260);
                        auto path=canonical(std::string(text,strnlen(text,260)));
                        if(!path.empty()) file_models.emplace(path,int32_t(i));
                    }
                    p=next+(n&1);
                }
            }
            at=end+(word(at+4)&1);
        }
    }
    void sanitize(EAXSOURCEPROPERTIES& p) {
        auto f=[](float v,float lo,float hi,float fallback){return std::isfinite(v)?std::clamp(v,lo,hi):fallback;};
        for(auto v:{&p.lDirect,&p.lRoom}) *v=std::clamp(*v,-10000,1000);
        for(auto v:{&p.lDirectHF,&p.lRoomHF,&p.lObstruction,&p.lOcclusion,&p.lExclusion,&p.lOutsideVolumeHF}) *v=std::clamp(*v,-10000,0);
        p.flObstructionLFRatio=f(p.flObstructionLFRatio,0,1,0);
        p.flOcclusionLFRatio=f(p.flOcclusionLFRatio,0,1,.25f);
        p.flOcclusionRoomRatio=f(p.flOcclusionRoomRatio,0,10,1.5f);
        p.flOcclusionDirectRatio=f(p.flOcclusionDirectRatio,0,10,1);
        p.flExclusionLFRatio=f(p.flExclusionLFRatio,0,1,1);
        p.flRoomRolloffFactor=f(p.flRoomRolloffFactor,0,10,0);
        p.flAirAbsorptionFactor=f(p.flAirAbsorptionFactor,0,10,0);
        p.ulFlags &= EAXSOURCEFLAGS_DIRECTHFAUTO | EAXSOURCEFLAGS_ROOMHFAUTO;
    }
    void read_apertures() {
        // Pair aperture endpoints by their acoustic environment, not instance order.
        for(int i=0;i<64;++i) {
            std::map<int,std::vector<EMPoint>> ends[2];
            bool valid=true;
            for(int side=0;side<2;++side) {
                int32_t id=0,count=0;
                const auto name="Aperture"+std::to_string(i)+(side?"b":"a");
                if(manager.GetSourceID(name,id)!=0 || manager.GetSourceNumInstances(id,count)!=0) { valid=false; break; }
                for(int j=0;j<count;++j) {
                    EMPoint p{}; int32_t env=-1;
                    if(manager.GetSourceInstancePos(id,j,p)!=0 || manager.GetListenerDynamicAttributes(0,p,env,0)!=0) continue;
                    ends[side][env].push_back(p);
                }
            }
            if(!valid) break;
            for(const auto& [env,a_list]:ends[0]) {
                auto found=ends[1].find(env); if(found==ends[1].end()) continue;
                // Several acoustic subspaces may share a preset ID. Retain
                // every instance pair in authored order instead of rejecting
                // duplicate preset IDs (present in retail t3_byss).
                const auto& b_list=found->second;
                for(size_t j=0;j<std::min(a_list.size(),b_list.size());++j) {
                    const auto a=a_list[j],b=b_list[j];
                    apertures[env].push_back({a,b,{(a.fX+b.fX)*.5f,(a.fY+b.fY)*.5f,(a.fZ+b.fZ)*.5f}});
                }
            }
        }
    }
    const Aperture* nearest_aperture(int env,EMPoint listener) const {
        auto it=apertures.find(env); if(it==apertures.end()) return nullptr;
        const Aperture* nearest=nullptr; float closest=1e30f;
        for(const auto& a:it->second) { const auto d=distance(a.center,listener); if(d<closest) { closest=d; nearest=&a; } }
        return nearest;
    }
    void rooms(const DjkEalFrame& frame,DjkEalStats& stats) {
        const auto listener=eagle(frame.listener);
        if(manager.GetListenerDynamicAttributes(0,listener,listener_environment,EMFLAG_LOCKPOSITION)!=0)
            throw std::runtime_error("EalMan listener query failed");
        std::vector<std::pair<float,int>> nearby;
        for(const auto& [env,_]:apertures) if(env!=listener_environment) {
            if(auto a=nearest_aperture(env,listener)) nearby.emplace_back(distance(a->center,listener),env);
        }
        // A room may have no aperture tags. Still give its audible sources
        // their own reverb whenever the finite EAX4 room budget permits.
        for(const auto& voice:frame.voices) if(voice.active) {
            int32_t env=-1;
            if(manager.GetListenerDynamicAttributes(0,eagle(voice.position),env,0)!=0 || env==listener_environment) continue;
            if(std::any_of(nearby.begin(),nearby.end(),[&](auto item){return item.second==env;})) continue;
            nearby.emplace_back(distance(eagle(voice.position),listener),env);
        }
        std::sort(nearby.begin(),nearby.end());
        std::array<int,3> wanted{{listener_environment,-999,-999}};
        for(size_t i=0;i<std::min<size_t>(2,nearby.size());++i) wanted[i+1]=nearby[i].second;
        for(int env:wanted) if(env!=-999 && std::find(environments.begin(),environments.end(),env)==environments.end()) {
            auto free=std::find_if(environments.begin(),environments.end(),[&](int e){return e==-999 || (e!=-998 && std::find(wanted.begin(),wanted.end(),e)==wanted.end());});
            if(free==environments.end()) throw std::runtime_error("No EAX room slot available");
            EAXListenerProperties properties{};
            if(manager.GetEnvironmentAttributes(env,properties)!=0) throw std::runtime_error("EalMan reverb query failed");
            static_assert(sizeof(properties)==sizeof(EAXREVERBPROPERTIES));
            const auto index=std::distance(environments.begin(),free);
            eax(slots[index],EAXREVERB_ALLPARAMETERS,0,properties);
            last_pan_valid[index]=false;
            *free=env;
        }
        auto primary=std::find(environments.begin(),environments.end(),listener_environment)-environments.begin();
        if(current_debug_mode!=3 || last_primary!=int(primary)) {
            eax(context_id,EAXCONTEXT_PRIMARYFXSLOTID,0,slots[primary]);
            last_primary=int(primary);
        }
        for(size_t i=0;i<environments.size();++i) {
            const int env=environments[i]; const bool active=env!=-999 && std::find(wanted.begin(),wanted.end(),env)!=wanted.end();
            const int32_t volume=active?0:-10000;
            if(current_debug_mode!=3 || last_volume[i]!=volume) {
                eax(slots[i],EAXFXSLOT_VOLUME,0,volume);
                last_volume[i]=volume;
            }
            room_active[i]=active;
            if(!active) continue;
            ++stats.rooms;
            auto aperture=nearest_aperture(env,listener);
            if(!aperture) continue; // Retain the authored preset pan in rooms without apertures.
            const float p1[3]={aperture->a.fX-listener.fX,aperture->a.fZ-listener.fZ,aperture->a.fY-listener.fY};
            const float p2[3]={aperture->b.fX-listener.fX,aperture->b.fZ-listener.fZ,aperture->b.fY-listener.fY};
            const float l1=std::sqrt(p1[0]*p1[0]+p1[1]*p1[1]+p1[2]*p1[2]);
            const float l2=std::sqrt(p2[0]*p2[0]+p2[1]*p2[1]+p2[2]*p2[2]);
            const float cosine=(l1*l2>1e-6f)?(p1[0]*p2[0]+p1[1]*p2[1]+p1[2]*p2[2])/(l1*l2):1.0f;
            const float angle=std::acos(std::clamp(cosine,-1.0f,1.0f));
            const float spread=angle>1e-5f?2.0f*std::sin(angle*0.5f)/angle:1.0f;
            const float magnitude=env==listener_environment?-(1.0f-spread):spread;
            const float delta[3]={aperture->center.fX-listener.fX,aperture->center.fZ-listener.fZ,aperture->center.fY-listener.fY};
            const float right[3]={frame.ahead[1]*frame.up[2]-frame.ahead[2]*frame.up[1],frame.ahead[2]*frame.up[0]-frame.ahead[0]*frame.up[2],frame.ahead[0]*frame.up[1]-frame.ahead[1]*frame.up[0]};
            auto dot=[&](const float* a){return delta[0]*a[0]+delta[1]*a[1]+delta[2]*a[2];};
            std::array<float,3> pan{{dot(right),dot(frame.up),-dot(frame.ahead)}};
            const float len=length(pan); if(len>1e-6f) for(auto& v:pan) v*=magnitude/len;
            else pan.fill(0.0f);
            EAXVECTOR value{pan[0],pan[1],pan[2]};
            if(current_debug_mode!=3 || !last_pan_valid[i] || std::memcmp(&last_pan[i],&value,sizeof(value))) {
                eax(slots[i],EAXREVERB_REFLECTIONSPAN,0,value);
                eax(slots[i],EAXREVERB_REVERBPAN,0,value);
                last_pan[i]=value;
                last_pan_valid[i]=true;
            }
            ++stats.directional_rooms;
        }
    }
    // Production EAX (mode 0) uses the clean callback transport when available.
    // Mode 9 explicitly retains the original queue for A/B regression testing.
    // Gracefully retain legacy queuing if an older OpenAL Soft lacks the extension.
    bool callback_reference() const { return current_debug_mode==7 || current_debug_mode==8 || (current_debug_mode==0 && callback_buffer); }
    bool dry_reference() const { return current_debug_mode==2 || current_debug_mode==4 || current_debug_mode==5 || current_debug_mode==6 || current_debug_mode==8; }
    void mark_unqueued(Source& s,ALuint b) {
        for(size_t i=0;i<s.buffers.size();++i) if(s.buffers[i]==b) {
            s.buffer_queued[i]=false;
            return;
        }
        throw std::runtime_error("EAL received unknown OpenAL buffer ID");
    }
    void stream(Source& s,const DjkEalVoice& voice,DjkEalStats& stats) {
        if(!s.source) { p_alGenSources(1,&s.source); p_alGenBuffers(3,s.buffers.data()); p_alSourcef(s.source,AL_ROLLOFF_FACTOR,0.0f); }
        if(s.id!=voice.id) {
            p_alSourceStop(s.source); p_alSourcei(s.source,AL_BUFFER,0); s.queued=0; s.next=0; s.id=voice.id;
            s.buffer_queued.fill(false); s.ever_started=false; s.pcm_previous_valid=false; s.tone_sample_index=0;
            s.callback_mode=callback_reference();
            s.callback_draining=false;
            s.callback_tail=0;
            s.callback_tone=current_debug_mode==8;
            s.callback_tone_sample=0;
            s.callback_rate=rate;
            s.callback_read=s.callback_used=0;
            s.telemetry=&telemetry;
            std::array<float,DJK_EAL_FRAMES> silence{};
            if(s.callback_mode) {
                // Keep EAX and positional AL rendering; only substitute the
                // input transport. Callback buffers are supported by OpenAL
                // Soft builds that also implement the current EAX backend.
                if(!callback_buffer) throw std::runtime_error("OpenAL Soft lacks AL_SOFT_callback_buffer; cannot run EAL debug mode 7/8");
                callback_buffer(s.buffers[0],AL_FORMAT_MONO_FLOAT32,ALsizei(rate),source_pcm_callback,&s);
                p_alSourcei(s.source,AL_BUFFER,ALint(s.buffers[0]));
                if(!s.callback_tone) callback_append(s,silence.data()); // Preserve one-block preroll for game PCM.
            } else {
                // Clear a prior callback registration before using this buffer
                // with the ordinary queue path again.
                // Modes 0-5 keep one queued silent block. Mode 6 keeps two.
                const unsigned preroll = current_debug_mode==6 ? 2u : 1u;
                for(unsigned i=0;i<preroll;++i) {
                    p_alBufferData(s.buffers[i],AL_FORMAT_MONO_FLOAT32,silence.data(),sizeof(silence),rate);
                    p_alSourceQueueBuffers(s.source,1,&s.buffers[i]);
                    s.buffer_queued[i]=true;
                    ++s.queued;
                }
                s.next=preroll;
            }
            s.properties_valid=false; s.send_valid=false; s.active_sends_valid=false; s.model={}; s.model.ulInsideConeAngle=360; s.model.ulOutsideConeAngle=360; s.model.fConeZdir=-1; s.base=default_properties;
            int32_t id=0;
            const auto name=canonical(std::string(voice.name,strnlen(voice.name,sizeof(voice.name))));
            const auto model=file_models.find(name);
            if(model!=file_models.end()) id=model->second;
            if(model!=file_models.end() || manager.GetSourceID(name,id)==0) {
                if(manager.GetSourceAttributes(id,s.model)==0) std::memcpy(&s.base,&s.model.eaxAttributes,sizeof(s.base));
            }
        }
        auto props=s.base;
        static_assert(sizeof(props)==sizeof(s.model.eaxAttributes));
        // PCM already carries JKA distance/volume and presentation pitch.
        props.flRolloffFactor=0.0f; props.flDopplerFactor=0.0f;
        sanitize(props);
        if(dry_reference()) {
            // Reference dry path: retain native OpenAL buffering, panning and
            // source time, but remove EAX sends and authored EAX filtering.
            props=default_properties;
            props.flRolloffFactor=0.0f;
            props.flDopplerFactor=0.0f;
            sanitize(props);
        }
        EMPoint virtual_position{};
        const auto position=eagle(voice.position);
        auto authored=props;
        if(!dry_reference() && manager.GetSourceDynamicAttributes(0,position,props.lObstruction,props.flObstructionLFRatio,
            props.lOcclusion,props.flOcclusionLFRatio,props.flOcclusionRoomRatio,virtual_position,0)!=0)
            throw std::runtime_error("EalMan source query failed");
        props.lObstruction=std::min(props.lObstruction,authored.lObstruction);
        props.lOcclusion=std::min(props.lOcclusion,authored.lOcclusion);
        if(authored.lObstruction) props.flObstructionLFRatio=std::max(props.flObstructionLFRatio,authored.flObstructionLFRatio);
        if(authored.lOcclusion) {
            props.flOcclusionLFRatio=std::max(props.flOcclusionLFRatio,authored.flOcclusionLFRatio);
            props.flOcclusionRoomRatio=std::max(props.flOcclusionRoomRatio,authored.flOcclusionRoomRatio);
        }
        // Compare only for diagnostics; rendering always uses diffraction-enabled results.
        int32_t ob=0,oc=0;float ob_lf=0,oc_lf=0,room=0;EMPoint ignored{};
        if(!dry_reference())
            manager.GetSourceDynamicAttributes(0,position,ob,ob_lf,oc,oc_lf,room,ignored,EMFLAG_NODIFFRACTION);
        if(std::min(oc,authored.lOcclusion)!=props.lOcclusion || std::min(ob,authored.lObstruction)!=props.lObstruction) ++stats.diffracted_sources;
        int32_t env=-1;
        if(!dry_reference()) manager.GetListenerDynamicAttributes(0,position,env,0);
        int routed=-1;
        for(int i=0;i<4;++i) if(room_active[i] && environments[i]==env) routed=i;
        const auto slot=routed<0?primary_id:slots[routed];
        EAXACTIVEFXSLOTS sends{};
        if(!dry_reference()) sends.guidActiveFXSlots[0]=slot;
        if(current_debug_mode!=3 || !s.active_sends_valid || std::memcmp(&sends,&s.active_sends,sizeof(sends))) {
            eax(source_id,EAXSOURCE_ACTIVEFXSLOTID,s.source,sends);
            s.active_sends=sends;
            s.active_sends_valid=true;
        }
        props.lExclusion=std::clamp(props.lExclusion+(env==listener_environment?0:-1000),-10000,0);
        if(props.lObstruction || props.lOcclusion || props.lDirectHF || props.lRoomHF || props.lExclusion) ++stats.filtered_sources;
        // Global EAX occlusion/exclusion only affect the primary send. Put
        // those attributes on the actual receiving room, including its direct
        // path ratio, so separate room routing cannot bypass the filters.
        if(!dry_reference()) {
            const auto primary=std::find(environments.begin(),environments.end(),listener_environment)-environments.begin();
            EAXSOURCEALLSENDPROPERTIES send{slots[routed<0?primary:routed],0,0,
                props.lOcclusion,props.flOcclusionLFRatio,props.flOcclusionRoomRatio,props.flOcclusionDirectRatio,
                props.lExclusion,props.flExclusionLFRatio};
            if(!s.send_valid || std::memcmp(&send,&s.send,sizeof(send))) {
                eax(source_id,EAXSOURCE_ALLSENDPARAMETERS,s.source,send);s.send=send;s.send_valid=true;
            }
        }
        props.lOcclusion=0;props.lExclusion=0;
        if(!s.properties_valid || std::memcmp(&props,&s.properties,sizeof(props))) {
            eax(source_id,EAXSOURCE_ALLPARAMETERS,s.source,props); s.properties=props; s.properties_valid=true;
        }
        // Mode 4 leaves OpenAL queue/loopback intact but removes spatialization.
        if(s.relative_mode!=(current_debug_mode==4 || current_debug_mode==8)) {
            p_alSourcei(s.source,AL_SOURCE_RELATIVE,(current_debug_mode==4 || current_debug_mode==8)?AL_TRUE:AL_FALSE);
            s.relative_mode=current_debug_mode==4 || current_debug_mode==8;
        }
        const auto alpos=openal(voice.position);
        p_alSource3f(s.source,AL_POSITION,(current_debug_mode==4 || current_debug_mode==8)?0.0f:alpos[0],(current_debug_mode==4 || current_debug_mode==8)?0.0f:alpos[1],(current_debug_mode==4 || current_debug_mode==8)?0.0f:alpos[2]);
        p_alSourcef(s.source,AL_CONE_INNER_ANGLE,dry_reference()?360.0f:std::clamp(float(s.model.ulInsideConeAngle),0.0f,360.0f));
        p_alSourcef(s.source,AL_CONE_OUTER_ANGLE,dry_reference()?360.0f:std::clamp(float(s.model.ulOutsideConeAngle),0.0f,360.0f));
        p_alSourcef(s.source,AL_CONE_OUTER_GAIN,dry_reference()?1.0f:std::clamp(std::pow(10.0f,s.model.lConeOutsideVolume/2000.0f),0.0f,1.0f));
        p_alSource3f(s.source,AL_DIRECTION,s.model.fConeXdir,s.model.fConeYdir,-s.model.fConeZdir);
        if(s.callback_mode) {
            const float* pcm=voice.pcm;
            std::array<float,DJK_EAL_FRAMES> tone{};
            if(current_debug_mode==8) {
                constexpr double pi=3.14159265358979323846;
                for(auto& sample:tone)
                    sample=0.025f*float(std::sin(2.0*pi*440.0*double(s.tone_sample_index++)/double(rate)));
                pcm=tone.data();
            }
            if(s.pcm_previous_valid)
                max_pcm_edge=std::max(max_pcm_edge,std::abs(pcm[0]-s.last_pcm));
            s.last_pcm=pcm[DJK_EAL_FRAMES-1];
            s.pcm_previous_valid=true;
            if(!s.callback_tone) callback_append(s,pcm);
            ALint state=0;p_alGetSourcei(s.source,AL_SOURCE_STATE,&state);
            if(state!=AL_PLAYING) {
                if(s.ever_started) ++unexpected_restarts;
                p_alSourcePlay(s.source);
                s.ever_started=true;
            }
            ++stats.sources;
            return;
        }
        ALint processed=0;p_alGetSourcei(s.source,AL_BUFFERS_PROCESSED,&processed);
        if(processed>1) ++multiple_processed;
        while(processed-->0 && s.queued>0) {
            ALuint b=0;
            p_alSourceUnqueueBuffers(s.source,1,&b);
            mark_unqueued(s,b);
            --s.queued;
        }
        if(s.ever_started && s.queued==0) ++queue_starves;
        if(s.queued>=3) throw std::runtime_error("EAX source streaming queue did not advance");
        const auto index=s.next;
        const auto buffer=s.buffers[index];s.next=(s.next+1)%3;
        if(s.buffer_queued[index]) ++buffer_reuse;
        const float* pcm=voice.pcm;
        std::array<float,DJK_EAL_FRAMES> tone{};
        if(current_debug_mode==5) {
            // A phase-continuous, deliberately low-amplitude source eliminates
            // decoder, asset-loop and upstream PCM-boundary variables.
            constexpr double pi=3.14159265358979323846;
            for(auto& sample:tone) {
                sample=0.025f*float(std::sin(2.0*pi*440.0*double(s.tone_sample_index++)/double(rate)));
            }
            pcm=tone.data();
        }
        if(s.pcm_previous_valid) max_pcm_edge=std::max(max_pcm_edge,std::abs(pcm[0]-s.last_pcm));
        s.last_pcm=pcm[DJK_EAL_FRAMES-1];
        s.pcm_previous_valid=true;
        p_alBufferData(buffer,AL_FORMAT_MONO_FLOAT32,pcm,sizeof(voice.pcm),rate);
        p_alSourceQueueBuffers(s.source,1,&buffer);s.buffer_queued[index]=true;++s.queued;
        ALint state=0;p_alGetSourcei(s.source,AL_SOURCE_STATE,&state);
        if(state!=AL_PLAYING) {
            if(s.ever_started) ++unexpected_restarts;
            p_alSourcePlay(s.source);
            s.ever_started=true;
        }
        ++stats.sources;
    }
    void process(const DjkEalFrame& frame,float* out,DjkEalStats& stats) {
        if(!make(context)) throw std::runtime_error("Cannot activate EAX scene context");
        if(requested_hrtf!=int(frame.hrtf)) {
            const ALCint attributes[]={ALC_FORMAT_CHANNELS_SOFT,ALC_STEREO_SOFT,ALC_FORMAT_TYPE_SOFT,ALC_FLOAT_SOFT,
                ALC_FREQUENCY,static_cast<ALCint>(rate),ALC_MAX_AUXILIARY_SENDS,4,ALC_HRTF_SOFT,frame.hrtf?ALC_TRUE:ALC_FALSE,0};
            if(!reset(device,attributes)) throw std::runtime_error("OpenAL could not apply EAL headphone positioning");
            requested_hrtf=frame.hrtf?1:0;
            ALCint active=ALC_FALSE;
            p_alcGetIntegerv(device,ALC_HRTF_SOFT,1,&active);
            p_alcGetIntegerv(device,ALC_HRTF_STATUS_SOFT,1,&hrtf_status);
            hrtf=active==ALC_TRUE?1:0;
        }
        if(last_debug_mode!=frame.debug_mode) {
            // Discard stale PCM from the old mode. Particularly important after
            // mode 1, which intentionally bypasses OpenAL render progression.
            for(auto& s:sources) if(s.source) {
                p_alSourceStop(s.source);
                p_alSourcei(s.source,AL_BUFFER,0);
                s.id=0; s.queued=0; s.next=0;
                s.buffer_queued.fill(false);
                s.callback_mode=false; s.callback_draining=false;
                s.callback_read=s.callback_used=0;
                s.telemetry=&telemetry;
                s.ever_started=false; s.pcm_previous_valid=false;
            }
            telemetry={};
            queue_starves=unexpected_restarts=multiple_processed=buffer_reuse=0;
            max_pcm_edge=max_output_edge=0.0f;
            previous_output_valid=false;
            current_debug_mode=frame.debug_mode;
            last_debug_mode=frame.debug_mode;
            last_primary=-1;
            last_volume.fill(INT32_MIN);
            last_pan_valid.fill(false);
            for(auto& src:sources) {
                src.active_sends_valid=false;
                src.send_valid=false;
                src.properties_valid=false;
            }
        }
        stats={}; stats.hrtf=hrtf; stats.hrtf_status=uint32_t(hrtf_status);
        stats.transport=callback_reference()?1u:2u;
        const auto pos=openal(frame.listener);p_alListenerfv(AL_POSITION,pos.data());
        const auto ahead=openal(frame.ahead),up=openal(frame.up);
        const float orientation[]={ahead[0],ahead[1],ahead[2],up[0],up[1],up[2]};p_alListenerfv(AL_ORIENTATION,orientation);
        if(!dry_reference()) rooms(frame,stats);
        for(size_t i=0;i<sources.size();++i) {
            if(frame.voices[i].active) stream(sources[i],frame.voices[i],stats);
            else if(sources[i].id) {
                if(sources[i].callback_mode) {
                    // Let the final submitted samples drain before disposing
                    // this native source, just as the legacy queue path does.
                    auto& s=sources[i];
                    if(!s.callback_draining) {
                        s.callback_draining=true;
                        s.callback_tail=DJK_EAL_FRAMES;
                    }
                    ALint state=0;p_alGetSourcei(s.source,AL_SOURCE_STATE,&state);
                    // FIFO consumption is not playback completion: OpenAL may
                    // have prefetched the final PCM. Let the callback return
                    // EOF and let AL finish before detaching its buffer.
                    if(state==AL_STOPPED) {
                        p_alSourcei(s.source,AL_BUFFER,0);
                        s.id=0;s.callback_mode=false;
                    }
                    continue;
                }
                // An inactive input still has its final PCM block queued.
                // Drain it before clearing this source; reverb keeps its tail.
                auto& s=sources[i];ALint processed=0;p_alGetSourcei(s.source,AL_BUFFERS_PROCESSED,&processed);
                while(processed-->0 && s.queued>0) {
                    ALuint b=0;p_alSourceUnqueueBuffers(s.source,1,&b);
                    mark_unqueued(s,b);--s.queued;
                }
                if(s.queued==0) {
                    p_alSourceStop(s.source);p_alSourcei(s.source,AL_BUFFER,0);
                    s.id=0;s.buffer_queued.fill(false);
                }
            }
        }
        render(device,out,DJK_EAL_FRAMES);check("EAX scene rendering");
        for(size_t i=0;i<DJK_EAL_FRAMES*2;++i) if(!std::isfinite(out[i])) throw std::runtime_error("EAX produced non-finite audio");
        if(previous_output_valid) for(int channel=0;channel<2;++channel)
            max_output_edge=std::max(max_output_edge,std::abs(out[channel]-previous_output[channel]));
        previous_output[0]=out[(DJK_EAL_FRAMES-1)*2];
        previous_output[1]=out[(DJK_EAL_FRAMES-1)*2+1];
        previous_output_valid=true;
        stats.queue_starves=uint32_t(std::min<uint64_t>(uint64_t(queue_starves)+telemetry.starves,UINT32_MAX));
        stats.callback_underfeed_samples=telemetry.underfeed_samples;
        stats.callback_dropped_samples=telemetry.dropped_samples;
        stats.restarts=unexpected_restarts;
        stats.multi_processed=multiple_processed;
        stats.buffer_reuse=buffer_reuse;
        stats.input_edge_max=max_pcm_edge;
        stats.output_edge_max=max_output_edge;
    }
};
}

extern "C" void* djk_eal_create(const uint16_t* dll,const uint8_t* bytes,uint32_t size,uint32_t rate,char* error,uint32_t capacity) {
    try { auto scene=std::make_unique<Scene>();scene->initialize(dll,bytes,size,rate);return scene.release(); }
    catch(const std::exception& e) { error_text(error,capacity,e.what());return nullptr; }
    catch(...) { error_text(error,capacity,"Unknown EAL initialization failure");return nullptr; }
}
extern "C" int djk_eal_render(void* scene,const DjkEalFrame* frame,float* stereo,DjkEalStats* stats,char* error,uint32_t capacity) {
    try { static_cast<Scene*>(scene)->process(*frame,stereo,*stats);return 1; }
    catch(const std::exception& e) { std::fill(stereo,stereo+DJK_EAL_FRAMES*2,0.0f);error_text(error,capacity,e.what());return 0; }
    catch(...) { std::fill(stereo,stereo+DJK_EAL_FRAMES*2,0.0f);error_text(error,capacity,"Unknown EAL render failure");return 0; }
}
extern "C" void djk_eal_destroy(void* scene) { delete static_cast<Scene*>(scene); }
