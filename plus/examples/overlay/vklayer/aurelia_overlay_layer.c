#define VK_NO_PROTOTYPES
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <vulkan/vulkan.h>
#include <vulkan/vk_layer.h>

#define SHM_BYTES (16u << 20)
#define HDR 64
#define SLOTS 8
#define EXPORT __attribute__((visibility("default")))

typedef struct { uint32_t magic, version, seq, visible, w, h, frame_w, frame_h; } Hdr;

typedef struct InstData { void *key; VkInstance inst; PFN_vkGetInstanceProcAddr gipa; PFN_vkDestroyInstance destroy; PFN_vkGetPhysicalDeviceMemoryProperties memprops; struct InstData *next; } InstData;
typedef struct Swap { VkSwapchainKHR sc; VkFormat fmt; VkExtent2D ext; uint32_t n; VkImage *images; struct Swap *next; } Swap;
typedef struct Queue { VkQueue q; uint32_t family; struct Queue *next; } Queue;
typedef struct DevData {
    void *key; VkDevice dev; VkPhysicalDevice phys; InstData *inst; PFN_vkGetDeviceProcAddr gdpa;
    PFN_vkDestroyDevice destroy; PFN_vkCreateSwapchainKHR create_sc; PFN_vkDestroySwapchainKHR destroy_sc; PFN_vkGetSwapchainImagesKHR get_images;
    PFN_vkQueuePresentKHR present; PFN_vkGetDeviceQueue get_queue; PFN_vkCreateCommandPool create_pool; PFN_vkAllocateCommandBuffers alloc_cb;
    PFN_vkBeginCommandBuffer begin; PFN_vkEndCommandBuffer end; PFN_vkCmdPipelineBarrier barrier; PFN_vkCmdCopyBufferToImage copy;
    PFN_vkQueueSubmit submit; PFN_vkCreateFence create_fence; PFN_vkWaitForFences wait_fences; PFN_vkResetFences reset_fences;
    PFN_vkCreateSemaphore create_sem; PFN_vkCreateBuffer create_buf; PFN_vkGetBufferMemoryRequirements buf_reqs; PFN_vkAllocateMemory alloc_mem;
    PFN_vkBindBufferMemory bind_buf; PFN_vkMapMemory map_mem; PFN_vkResetCommandBuffer reset_cb;
    PFN_vkSetDeviceLoaderData set_loader;
    Swap *swaps; Queue *queues;
    int ready; VkCommandPool pool; VkCommandBuffer cb[SLOTS]; VkFence fence[SLOTS]; VkSemaphore sem[SLOTS]; int fence_used[SLOTS]; uint32_t slot;
    VkBuffer buf; VkDeviceMemory mem; uint8_t *mapped; uint32_t pool_family; uint32_t last_seq; VkFormat last_fmt;
    struct DevData *next;
} DevData;

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static InstData *g_insts; static DevData *g_devs;
static Hdr *g_shm; static int g_shm_tried; static unsigned g_frame;

static int g_debug = -1;
#define DBG(...) do { if (g_debug < 0) g_debug = getenv("AURELIA_OVERLAY_DEBUG") != NULL; if (g_debug) { fprintf(stderr, "[aurelia-overlay] " __VA_ARGS__); fputc('\n', stderr); } } while (0)

static void *key_of(void *h) { return *(void **)h; }
static InstData *inst_of(void *h) { InstData *r = NULL; pthread_mutex_lock(&g_lock); for (InstData *i = g_insts; i; i = i->next) if (i->key == key_of(h)) r = i; pthread_mutex_unlock(&g_lock); return r; }
static DevData *dev_of(void *h) { DevData *r = NULL; pthread_mutex_lock(&g_lock); for (DevData *d = g_devs; d; d = d->next) if (d->key == key_of(h)) r = d; pthread_mutex_unlock(&g_lock); return r; }

static void open_shm(void) {
    const char *path = getenv("AURELIA_OVERLAY_SHM");
    if (!path || !*path) { g_shm_tried = 1 << 30; return; }
    int fd = open(path, O_RDWR);
    if (fd < 0) return;
    struct stat st;
    if (fstat(fd, &st) == 0 && (uint32_t)st.st_size >= SHM_BYTES) {
        void *p = mmap(NULL, SHM_BYTES, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (p != MAP_FAILED) g_shm = p;
    }
    close(fd);
}

static VKAPI_ATTR VkResult VKAPI_CALL layer_CreateInstance(const VkInstanceCreateInfo *ci, const VkAllocationCallbacks *ac, VkInstance *out) {
    VkLayerInstanceCreateInfo *chain = (VkLayerInstanceCreateInfo *)ci->pNext;
    while (chain && !(chain->sType == VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO && chain->function == VK_LAYER_LINK_INFO)) chain = (VkLayerInstanceCreateInfo *)chain->pNext;
    if (!chain) return VK_ERROR_INITIALIZATION_FAILED;
    PFN_vkGetInstanceProcAddr gipa = chain->u.pLayerInfo->pfnNextGetInstanceProcAddr;
    chain->u.pLayerInfo = chain->u.pLayerInfo->pNext;
    PFN_vkCreateInstance create = (PFN_vkCreateInstance)gipa(VK_NULL_HANDLE, "vkCreateInstance");
    VkResult r = create(ci, ac, out);
    if (r != VK_SUCCESS) return r;
    InstData *d = calloc(1, sizeof *d);
    d->key = key_of(*out); d->inst = *out; d->gipa = gipa;
    d->destroy = (PFN_vkDestroyInstance)gipa(*out, "vkDestroyInstance");
    d->memprops = (PFN_vkGetPhysicalDeviceMemoryProperties)gipa(*out, "vkGetPhysicalDeviceMemoryProperties");
    pthread_mutex_lock(&g_lock); d->next = g_insts; g_insts = d; pthread_mutex_unlock(&g_lock);
    return VK_SUCCESS;
}

static VKAPI_ATTR void VKAPI_CALL layer_DestroyInstance(VkInstance inst, const VkAllocationCallbacks *ac) {
    InstData *d = inst_of(inst);
    PFN_vkDestroyInstance destroy = d->destroy;
    pthread_mutex_lock(&g_lock);
    for (InstData **p = &g_insts; *p; p = &(*p)->next) if (*p == d) { *p = d->next; break; }
    pthread_mutex_unlock(&g_lock);
    free(d);
    destroy(inst, ac);
}

#define LOAD(field, type, name) d->field = (type)d->gdpa(*out, name)
static VKAPI_ATTR VkResult VKAPI_CALL layer_CreateDevice(VkPhysicalDevice phys, const VkDeviceCreateInfo *ci, const VkAllocationCallbacks *ac, VkDevice *out) {
    VkLayerDeviceCreateInfo *chain = (VkLayerDeviceCreateInfo *)ci->pNext, *loader = NULL;
    while (chain && !(chain->sType == VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO && chain->function == VK_LAYER_LINK_INFO)) chain = (VkLayerDeviceCreateInfo *)chain->pNext;
    for (VkLayerDeviceCreateInfo *c = (VkLayerDeviceCreateInfo *)ci->pNext; c; c = (VkLayerDeviceCreateInfo *)c->pNext)
        if (c->sType == VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO && c->function == VK_LOADER_DATA_CALLBACK) loader = c;
    if (!chain) return VK_ERROR_INITIALIZATION_FAILED;
    PFN_vkGetInstanceProcAddr gipa = chain->u.pLayerInfo->pfnNextGetInstanceProcAddr;
    PFN_vkGetDeviceProcAddr gdpa = chain->u.pLayerInfo->pfnNextGetDeviceProcAddr;
    chain->u.pLayerInfo = chain->u.pLayerInfo->pNext;
    InstData *inst = g_insts;
    PFN_vkCreateDevice create = (PFN_vkCreateDevice)gipa(inst ? inst->inst : VK_NULL_HANDLE, "vkCreateDevice");
    VkResult r = create(phys, ci, ac, out);
    if (r != VK_SUCCESS) return r;
    DevData *d = calloc(1, sizeof *d);
    d->key = key_of(*out); d->dev = *out; d->phys = phys; d->inst = inst; d->gdpa = gdpa;
    d->set_loader = loader ? loader->u.pfnSetDeviceLoaderData : NULL;
    LOAD(destroy, PFN_vkDestroyDevice, "vkDestroyDevice"); LOAD(create_sc, PFN_vkCreateSwapchainKHR, "vkCreateSwapchainKHR");
    LOAD(destroy_sc, PFN_vkDestroySwapchainKHR, "vkDestroySwapchainKHR"); LOAD(get_images, PFN_vkGetSwapchainImagesKHR, "vkGetSwapchainImagesKHR");
    LOAD(present, PFN_vkQueuePresentKHR, "vkQueuePresentKHR"); LOAD(get_queue, PFN_vkGetDeviceQueue, "vkGetDeviceQueue");
    LOAD(create_pool, PFN_vkCreateCommandPool, "vkCreateCommandPool"); LOAD(alloc_cb, PFN_vkAllocateCommandBuffers, "vkAllocateCommandBuffers");
    LOAD(begin, PFN_vkBeginCommandBuffer, "vkBeginCommandBuffer"); LOAD(end, PFN_vkEndCommandBuffer, "vkEndCommandBuffer");
    LOAD(barrier, PFN_vkCmdPipelineBarrier, "vkCmdPipelineBarrier"); LOAD(copy, PFN_vkCmdCopyBufferToImage, "vkCmdCopyBufferToImage");
    LOAD(submit, PFN_vkQueueSubmit, "vkQueueSubmit"); LOAD(create_fence, PFN_vkCreateFence, "vkCreateFence");
    LOAD(wait_fences, PFN_vkWaitForFences, "vkWaitForFences"); LOAD(reset_fences, PFN_vkResetFences, "vkResetFences");
    LOAD(create_sem, PFN_vkCreateSemaphore, "vkCreateSemaphore"); LOAD(create_buf, PFN_vkCreateBuffer, "vkCreateBuffer");
    LOAD(buf_reqs, PFN_vkGetBufferMemoryRequirements, "vkGetBufferMemoryRequirements"); LOAD(alloc_mem, PFN_vkAllocateMemory, "vkAllocateMemory");
    LOAD(bind_buf, PFN_vkBindBufferMemory, "vkBindBufferMemory"); LOAD(map_mem, PFN_vkMapMemory, "vkMapMemory"); LOAD(reset_cb, PFN_vkResetCommandBuffer, "vkResetCommandBuffer");
    pthread_mutex_lock(&g_lock); d->next = g_devs; g_devs = d; pthread_mutex_unlock(&g_lock);
    return VK_SUCCESS;
}

static VKAPI_ATTR void VKAPI_CALL layer_DestroyDevice(VkDevice dev, const VkAllocationCallbacks *ac) {
    DevData *d = dev_of(dev);
    PFN_vkDestroyDevice destroy = d->destroy;
    pthread_mutex_lock(&g_lock);
    for (DevData **p = &g_devs; *p; p = &(*p)->next) if (*p == d) { *p = d->next; break; }
    pthread_mutex_unlock(&g_lock);
    destroy(dev, ac);
}

static VKAPI_ATTR void VKAPI_CALL layer_GetDeviceQueue(VkDevice dev, uint32_t family, uint32_t index, VkQueue *q) {
    DevData *d = dev_of(dev);
    d->get_queue(dev, family, index, q);
    if (*q) { Queue *e = calloc(1, sizeof *e); e->q = *q; e->family = family; e->next = d->queues; d->queues = e; }
}

static VKAPI_ATTR VkResult VKAPI_CALL layer_CreateSwapchain(VkDevice dev, const VkSwapchainCreateInfoKHR *ci, const VkAllocationCallbacks *ac, VkSwapchainKHR *out) {
    DevData *d = dev_of(dev);
    VkSwapchainCreateInfoKHR copy = *ci;
    copy.imageUsage |= VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    VkResult r = d->create_sc(dev, &copy, ac, out);
    if (r != VK_SUCCESS) return r;
    Swap *s = calloc(1, sizeof *s);
    s->sc = *out; s->fmt = ci->imageFormat; s->ext = ci->imageExtent;
    d->get_images(dev, *out, &s->n, NULL);
    s->images = calloc(s->n, sizeof *s->images);
    d->get_images(dev, *out, &s->n, s->images);
    s->next = d->swaps; d->swaps = s;
    return VK_SUCCESS;
}

static VKAPI_ATTR void VKAPI_CALL layer_DestroySwapchain(VkDevice dev, VkSwapchainKHR sc, const VkAllocationCallbacks *ac) {
    DevData *d = dev_of(dev);
    for (Swap **p = &d->swaps; *p; p = &(*p)->next) if ((*p)->sc == sc) { Swap *s = *p; *p = s->next; free(s->images); free(s); break; }
    d->destroy_sc(dev, sc, ac);
}

static int format_ok(VkFormat f) {
    return f == VK_FORMAT_B8G8R8A8_UNORM || f == VK_FORMAT_B8G8R8A8_SRGB || f == VK_FORMAT_R8G8B8A8_UNORM || f == VK_FORMAT_R8G8B8A8_SRGB ||
           f == VK_FORMAT_A2B10G10R10_UNORM_PACK32 || f == VK_FORMAT_A2R10G10B10_UNORM_PACK32;
}
static void convert(uint8_t *dst, const uint8_t *src, size_t npx, VkFormat f) {
    if (f == VK_FORMAT_B8G8R8A8_UNORM || f == VK_FORMAT_B8G8R8A8_SRGB) { memcpy(dst, src, npx * 4); return; }
    for (size_t i = 0; i < npx; i++) {
        uint32_t b = src[i * 4], g = src[i * 4 + 1], r = src[i * 4 + 2], a = src[i * 4 + 3];
        if (f == VK_FORMAT_R8G8B8A8_UNORM || f == VK_FORMAT_R8G8B8A8_SRGB) { dst[i * 4] = (uint8_t)r; dst[i * 4 + 1] = (uint8_t)g; dst[i * 4 + 2] = (uint8_t)b; dst[i * 4 + 3] = (uint8_t)a; continue; }
        uint32_t r10 = (r << 2) | (r >> 6), g10 = (g << 2) | (g >> 6), b10 = (b << 2) | (b >> 6), a2 = a >> 6, v;
        v = f == VK_FORMAT_A2B10G10R10_UNORM_PACK32 ? (a2 << 30) | (b10 << 20) | (g10 << 10) | r10 : (a2 << 30) | (r10 << 20) | (g10 << 10) | b10;
        memcpy(dst + i * 4, &v, 4);
    }
}

static int pick_memory(DevData *d, uint32_t bits) {
    VkPhysicalDeviceMemoryProperties mp;
    d->inst->memprops(d->phys, &mp);
    for (uint32_t i = 0; i < mp.memoryTypeCount; i++)
        if ((bits & (1u << i)) && (mp.memoryTypes[i].propertyFlags & (VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)) == (VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)) return (int)i;
    return -1;
}

static int setup(DevData *d, uint32_t family) {
    VkBufferCreateInfo bi = { VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO, NULL, 0, SHM_BYTES - HDR, VK_BUFFER_USAGE_TRANSFER_SRC_BIT, VK_SHARING_MODE_EXCLUSIVE };
    if (d->create_buf(d->dev, &bi, NULL, &d->buf) != VK_SUCCESS) return 0;
    VkMemoryRequirements rq;
    d->buf_reqs(d->dev, d->buf, &rq);
    int type = pick_memory(d, rq.memoryTypeBits);
    if (type < 0) return 0;
    VkMemoryAllocateInfo mi = { VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, NULL, rq.size, (uint32_t)type };
    if (d->alloc_mem(d->dev, &mi, NULL, &d->mem) != VK_SUCCESS || d->bind_buf(d->dev, d->buf, d->mem, 0) != VK_SUCCESS) return 0;
    void *p;
    if (d->map_mem(d->dev, d->mem, 0, VK_WHOLE_SIZE, 0, &p) != VK_SUCCESS) return 0;
    d->mapped = p;
    VkCommandPoolCreateInfo pi = { VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO, NULL, VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT, family };
    if (d->create_pool(d->dev, &pi, NULL, &d->pool) != VK_SUCCESS) return 0;
    VkCommandBufferAllocateInfo ai = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO, NULL, d->pool, VK_COMMAND_BUFFER_LEVEL_PRIMARY, SLOTS };
    if (d->alloc_cb(d->dev, &ai, d->cb) != VK_SUCCESS) return 0;
    for (int i = 0; i < SLOTS; i++) {
        if (d->set_loader) d->set_loader(d->dev, d->cb[i]);
        VkFenceCreateInfo fi = { VK_STRUCTURE_TYPE_FENCE_CREATE_INFO };
        VkSemaphoreCreateInfo si = { VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO };
        if (d->create_fence(d->dev, &fi, NULL, &d->fence[i]) != VK_SUCCESS || d->create_sem(d->dev, &si, NULL, &d->sem[i]) != VK_SUCCESS) return 0;
    }
    d->pool_family = family;
    return 1;
}

static VkSemaphore compose(DevData *d, VkQueue queue, const VkPresentInfoKHR *pi) {
    if (!g_shm && g_shm_tried < (1 << 29) && (g_frame++ % 60) == 0) { open_shm(); g_shm_tried++; }
    if (!g_shm) { if (g_frame % 120 == 1) DBG("no shared memory (AURELIA_OVERLAY_SHM)"); return VK_NULL_HANDLE; }
    if (g_shm->magic != 0x4C564F41  || !g_shm->visible || !g_shm->w || !g_shm->h || (uint64_t)g_shm->w * g_shm->h * 4 > SHM_BYTES - HDR) {
        static unsigned n; if (n++ % 300 == 0) DBG("overlay invisible or invalid header (magic=%x visible=%u %ux%u)", g_shm->magic, g_shm->visible, g_shm->w, g_shm->h);
        return VK_NULL_HANDLE;
    }
    Swap *s = NULL;
    for (Swap *x = d->swaps; x; x = x->next) if (x->sc == pi->pSwapchains[0]) s = x;
    if (!s || !format_ok(s->fmt)) { DBG("swapchain %s (format %d)", s ? "with unsupported format" : "unknown", s ? (int)s->fmt : -1); return VK_NULL_HANDLE; }
    uint32_t family = 0;
    for (Queue *q = d->queues; q; q = q->next) if (q->q == queue) family = q->family;
    if (!d->ready) { d->ready = setup(d, family) ? 1 : -1; }
    if (d->ready < 0) { DBG("failed to prepare the resources"); return VK_NULL_HANDLE; }
    static int once; if (!once++) DBG("compositing the overlay %ux%u onto the frame %ux%u", g_shm->w, g_shm->h, s->ext.width, s->ext.height);
    g_shm->frame_w = s->ext.width; g_shm->frame_h = s->ext.height;
    uint32_t w = g_shm->w < s->ext.width ? g_shm->w : s->ext.width, h = g_shm->h < s->ext.height ? g_shm->h : s->ext.height;
    uint32_t src_w = g_shm->w;
    uint32_t seq = g_shm->seq;
    if (seq != d->last_seq || d->last_fmt != s->fmt) { convert(d->mapped, (uint8_t *)g_shm + HDR, (size_t)g_shm->w * g_shm->h, s->fmt); d->last_seq = seq; d->last_fmt = s->fmt; }
    uint32_t slot = d->slot++ % SLOTS, idx = pi->pImageIndices[0];
    if (d->fence_used[slot]) { d->wait_fences(d->dev, 1, &d->fence[slot], VK_TRUE, UINT64_MAX); d->reset_fences(d->dev, 1, &d->fence[slot]); }
    VkCommandBuffer cb = d->cb[slot];
    d->reset_cb(cb, 0);
    VkCommandBufferBeginInfo bi = { VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO, NULL, VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT };
    d->begin(cb, &bi);
    VkImageMemoryBarrier b = { VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER, NULL, VK_ACCESS_MEMORY_READ_BIT, VK_ACCESS_TRANSFER_WRITE_BIT, VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
        VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED, s->images[idx], { VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1 } };
    d->barrier(cb, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, NULL, 0, NULL, 1, &b);
    VkBufferImageCopy region = { 0, src_w, g_shm->h, { VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1 }, { (int32_t)((s->ext.width - w) / 2), (int32_t)((s->ext.height - h) / 2), 0 }, { w, h, 1 } };
    d->copy(cb, d->buf, s->images[idx], VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region);
    b.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT; b.dstAccessMask = VK_ACCESS_MEMORY_READ_BIT; b.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL; b.newLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
    d->barrier(cb, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, NULL, 0, NULL, 1, &b);
    d->end(cb);
    VkPipelineStageFlags stages[16];
    uint32_t nw = pi->waitSemaphoreCount < 16 ? pi->waitSemaphoreCount : 16;
    for (uint32_t i = 0; i < nw; i++) stages[i] = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
    VkSubmitInfo si = { VK_STRUCTURE_TYPE_SUBMIT_INFO, NULL, nw, pi->pWaitSemaphores, stages, 1, &cb, 1, &d->sem[slot] };
    if (d->submit(queue, 1, &si, d->fence[slot]) != VK_SUCCESS) return VK_NULL_HANDLE;
    d->fence_used[slot] = 1;
    return d->sem[slot];
}

static VKAPI_ATTR VkResult VKAPI_CALL layer_QueuePresent(VkQueue queue, const VkPresentInfoKHR *pi) {
    DevData *d = dev_of(queue);
    static int first; if (!first++) DBG("first vkQueuePresentKHR (device %s)", d ? "known" : "UNKNOWN");
    if (!d) return VK_ERROR_DEVICE_LOST;
    VkSemaphore sem = pi->swapchainCount == 1 ? compose(d, queue, pi) : VK_NULL_HANDLE;
    if (sem == VK_NULL_HANDLE) return d->present(queue, pi);
    VkPresentInfoKHR copy = *pi;
    copy.waitSemaphoreCount = 1; copy.pWaitSemaphores = &sem;
    return d->present(queue, &copy);
}

#define HOOK(n) if (!strcmp(name, "vk" #n)) return (PFN_vkVoidFunction)layer_##n
static VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL layer_GetDeviceProcAddr(VkDevice dev, const char *name) {
    HOOK(DestroyDevice); HOOK(GetDeviceQueue);
    if (!strcmp(name, "vkCreateSwapchainKHR")) return (PFN_vkVoidFunction)layer_CreateSwapchain;
    if (!strcmp(name, "vkDestroySwapchainKHR")) return (PFN_vkVoidFunction)layer_DestroySwapchain;
    if (!strcmp(name, "vkQueuePresentKHR")) return (PFN_vkVoidFunction)layer_QueuePresent;
    DevData *d = dev ? dev_of(dev) : NULL;
    return d && d->gdpa ? d->gdpa(dev, name) : NULL;
}

static VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL layer_GetInstanceProcAddr(VkInstance inst, const char *name) {
    HOOK(CreateInstance); HOOK(DestroyInstance); HOOK(CreateDevice);
    if (!strcmp(name, "vkGetInstanceProcAddr")) return (PFN_vkVoidFunction)layer_GetInstanceProcAddr;
    if (!strcmp(name, "vkGetDeviceProcAddr")) return (PFN_vkVoidFunction)layer_GetDeviceProcAddr;
    PFN_vkVoidFunction dev = layer_GetDeviceProcAddr(VK_NULL_HANDLE, name);
    if (dev && (!strcmp(name, "vkCreateSwapchainKHR") || !strcmp(name, "vkDestroySwapchainKHR") || !strcmp(name, "vkQueuePresentKHR") || !strcmp(name, "vkGetDeviceQueue") || !strcmp(name, "vkDestroyDevice"))) return dev;
    InstData *i = inst ? inst_of(inst) : NULL;
    return i && i->gipa ? i->gipa(inst, name) : NULL;
}

EXPORT VKAPI_ATTR VkResult VKAPI_CALL vkNegotiateLoaderLayerInterfaceVersion(VkNegotiateLayerInterface *v) {
    if (v->loaderLayerInterfaceVersion >= 2) {
        v->pfnGetInstanceProcAddr = layer_GetInstanceProcAddr;
        v->pfnGetDeviceProcAddr = layer_GetDeviceProcAddr;
        v->pfnGetPhysicalDeviceProcAddr = NULL;
    }
    if (v->loaderLayerInterfaceVersion > 2) v->loaderLayerInterfaceVersion = 2;
    return VK_SUCCESS;
}
