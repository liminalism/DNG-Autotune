<!-- Converted from kronander2013.pdf — 9 pages -->

## Page 1

### Unified HDR reconstruction from raw CFA data
† † ‡ Joel Kronander , Stefan Gustavson , Gerhard Bonnet , Jonas Unger† † Link¨oping University‡SpheronVR AG
joel.kronander@liu.se
Abstract from any set of varying exposures into a seamless HDR im-age (HDR assembly). HDR reconstruction from multiple exposures poses sev-The vast majority of recent HDR reconstruction meth-eral challenges. Previous HDR reconstruction techniques ods, e.g. [2, 10, 32], have considered resampling and demo-have considered debayering, denoising, resampling (align-saicing as separate steps to be performed either before or af-ment) and exposure fusion in several steps. We instead ter HDR assembly. There are several problems with this ap-present a unifying approach, performing HDR assembly di-proach. Demosaicing before HDR assembly, e.g. perform-rectly from raw sensor data in a single processing opera-ing classic exposure bracketing with a regular LDR camera tion. Our algorithm includes a spatially adaptive HDR re-as in [6], causes problems with bad or missing data around construction based on fitting local polynomial approxima-saturated pixels. This is especially problematic as color tions to observed sensor data, using a localized likelihood channels saturate at different levels. On the other hand, approach incorporating spatially varying sensor noise. We demosaicing after HDR assembly, as performed by [2, 32] also present a realistic camera noise model adapted to HDR causes problems with blur and ghosting unless the sensors video. The method allows reconstruction to an arbitrary are perfectly aligned. For multi-sensor systems using high resolution and output mapping. We present an implemen-resolution sensors, it is problematic and costly, sometimes tation in CUDA and show real-time performance for an ex-even impossible, to exactly match the CFA patterns of the perimental 4 Mpixel multi-sensor HDR video system. We sensors. Tocci et al. [32] report misalignment errors around further show that our algorithm has clear advantages over the size of a pixel despite considerable alignment efforts. state-of-the-art methods, both in terms of flexibility and re-For exposure bracketing sequences, misalignment may re-construction quality. sult from camera shake. Hence, overlapping pixels from different sensors might not be of the same color. Further-more, treating demosaicing, resampling and HDR assem-
### 1. Introduction
bly as separate problems makes it difficult to analyze and HDR imaging enables a wide range of new applications incorporate sensor noise and other pixel characteristics in in image processing, computer graphics and cinematogra- the HDR reconstruction in a coherent and formal manner. phy. It is a field currently in rapid development, spurred by Recently some methods performing joint denoising and de-dramatic improvements over the last few years in digital im- mosaicing, joint super-resolution and HDR assembly and age sensors and camera systems. However, state-of-the-art joint demosaicing and HDR assembly have been proposed, sensor technology is still unable to capture the full dynamic see section 2.
range in a natural scene with enough precision in a single The main contribution of this article is a new approach exposure. There is thus a need to develop new HDR re- addressing all of these problems. The basis of the work is an construction algorithms that are general, flexible, of high adaptive spatial and cross-sensor filtering using a local poly-quality and fast enough for routine use. In particular HDR nomial approximation (LPA) based on a noise-aware maxi-video [17, 1, 35, 34, 32] involves large amounts of data, and mum likelihood estimation considering the heteroskedastic real-time processing is required for a proper viewfinder and sensor noise. During reconstruction, we consider the entire other interactive feedback to the camera operator. set of valid, non-saturated LDR samples available around Robust HDR image reconstruction operating on the raw a certain point, and perform CFA interpolation, resampling output from the sensor(s) should: perform conversion from and HDR assembly in a single operation. This leads to a color filter array (CFA) sampled data to color images (de- reconstruction framework that is flexible, robust, and appli-mosaicing), allow and compensate for misalignments be- cable to a wide range of multi-sensor and multi-exposure tween the exposures (resampling) and fuse LDR images capture systems. A key feature of our algorithm is its com-

---

## Page 2

putational speed, making it applicable also to real-time as-sembly of high resolution HDR video. Our CUDA imple-mentation on a Geforce 680 GPU is capable of reconstruct-ing 4 Mpixel HDR frames at a sustained rate of 25 fps. As a part of the framework we have also developed a sensor noise model tailored to our example applications. To re-construct color images, we reconstruct each color channel independently. Thus, unlike more advanced demosaicing methods [11], our current implementation does not utilize cross-correlation between channels to improve the estimate. We demonstrate the benefits of our approach on simu-lated data from a range of sensor setups, including the re-cently proposed hardware setup of Tocci et al. [32], as well as on data obtained from our own experimental multi-sensor HDR video camera. The supplementary material attached to this paper contains all HDR images used in the compar-isons as well as processed video examples.
### 2. Related Work
The algorithms presented in this paper is related to a large body of previous work, ranging from HDR capture and reconstruction, e.g. [29, 27], to theory and algorithms for accurate image reconstruction and image fusion, e.g. [5]. In this section, we give an overview of the previous work most closely related to the methods proposed in this paper. Non-parametric Image Processing - The last decade has seen an increased popularity of image processing op-erations using locally adaptive filter weights, for applica-tions in e.g. interpolation, denoising and upsampling. Ex-amples include normalized convolution [20], the bilateral filter [33], and moving least squares [21]. Recently, deep connections have been shown [30, 25] between these meth-ods and traditional non-parametric statistics [22]. In this paper, we fit Local Polynomial Approximations (LPA) [5] to irregularly distributed samples around output pixels using a localized maximum likelihood estimation [31] to incorpo-rate the heterogeneous noise of the samples. HDR fusion - The most common method for HDR re-construction from a set of differently exposed Low Dy-namic Range (LDR) images is to compute a per-pixel weighted average of the LDR measurements. The weights, often based on heuristics, are chosen to suppress im-age noise and to remove saturated values from process-ing [23, 6, 3, 18] . Weight functions can also be based on more sophisticated camera noise models. Mitsunaga and Nayar [26] derived a weight function that maximizes SNR assuming signal-independent additive noise. Kirk and An-dersen [19] derived a weight function inversely proportional to the temporal variance of the digital LDR values. Grana-dos et al. [10] later extended this approach to include both spatial and temporal camera noise. While most previous methods consider only a single pixel at a time from each LDR exposure, Tocci et al. [32] presented an algorithm that
incorporates a neighbourhood of LDR samples in the recon-struction. This method helps smooth the transition regions between sensors and performs well for sensors with large exposure differences, over 3 f-stops apart. Image Fusion- When multiple images of the same scene are captured within sub-pixel displacements of each other, super-resolution reconstruction methods can be applied. A class of computationally simple super-resolution techniques are those based on interpolation in a common high resolu-tion grid [24]. These methods typically first use some reg-istration to put observed images in a common high reso-lution reference frame. The high-resolution image is then estimated in a least squares sense given a local neighbour-hood of observed pixels [8, 13]. As noted in previous work, super-resolution and HDR imaging complement each other. Super-resolution increases the resolution in the spatial do-main, and HDR increases the radiometric range and reso-lution. Several methods have also been proposed to incor-porate both enhancements in the same processing frame-work [12, 37]. Most modern camera systems acquire color images using a sensor covered by a color filter array, and estimates the final color vector for each pixel by demo-saicing, interpolation of neighboring color samples. For an overview of demosaicing, see Gunturk et al. [11]. Joint denoising and demosaicing have been considered in previ-ous work [16, 15]. Farsiu et al. [7] proposed a framework for joint super-resolution and demosaicing based on a max-imum a posteriori (MAP) optimization over all pixels in the output image, considering priors that enforce correlation be-tween color channels. A similar approach was taken in [4] for HDR imaging with joint demosaicing for sensors with non-destructive readout (ie perfect alignment between con-secutive readouts/frames). While a global MAP optimiza-tion could possibly also be used for joint realignment, HDR fusion and demosaicing, a global optimization is compu-tationally demanding and does not straightforwardly allow for parallel processing. To the authors knowledge, no previous approach has considered HDR assembly with joint realignment, noise re-duction and CFA interpolation. While our method is not designed to provide optimal results with respect to denois-ing, demosaicing or super-resolution alone, we take a uni-fied approach based on a model of sensor noise, providing a flexible framework which is straightforward to implement and use. Our method is also well suited to the needs of a versatile high dynamic range video system, by allowing an arbitrary resolution for the reconstruction and fast parallel processing.
### 3. Image Formation Model
Given a set of N differently exposed CFA images, Iss = 1:::N, the goal is to reconstruct an HDR frame, F. The images, Is, could for example be separate sensor im-

---

## Page 3

Figure 1: HDR reconstruction is performed in a virtual ref-erence space with arbitrary resolution and mapping (orange grid). Measured image pixels are mapped to this grid by transformations Ts. A pixel F(Xj) in the HDR image (or-ange cross) is estimated by a local polynomial approxima-tion of nearby samples (within the shaded circle). Saturated and defective samples are discarded, and samples near the black level, local outliers and samples farther from the re-construction point contribute less to the radiance estimate.
ages from a digital multi-sensor imaging system, or a set of still images captured using multi-exposure techniques. We make the following assumptions about the imaging system: – For multi-sensor systems, the sensors are synchronized and use the same exposure time, or the scene is assumed to be static. For multi-exposure systems, the scene is assumed to be static, and the camera is assumed to be static enough to avoid parallax. – The sensor images view the scene through a common opti-cal system implying same focus, point spread function, and vignetting for all sensor images. – Geometric misalignments between the sensor images can be described by some 2-D transformation Ts, so that they can be registered to a common frame of reference. – The parameters that are allowed to vary between the sen-sor images are: the exposure time ts, the gain setting gs; and an optional exposure scaling coefficient, ns.
### 4. Reconstruction Algorithm
Each sensor image, Is, samples the incident radiant power, f, at a set of discrete pixel locations. Using a lin-ear index i for pixels in each sensor image, we define the measured digital sample value at a pixel i in image Isas ys,i. The samples, ys,i, contain measurement noise that is dependent on the input signal, f, as well as the sensor char-acteristics. After performing radiometric and geometric cal-ibration of the sensors, the final HDR image, F, is estimated in one joint filtering operation directly from raw data.
4.1. Radiometric Calibration
f2ˆs,i. Similarly to previous work [10] we assume the noise
to follow a Gaussian distribution. More details on this as-sumption are given in Section 5, where we describe our ra-diometric noise model adapted for HDR video. Our frame-
![Figure 1](kronander2013_p3_figure1.jpg)
work can use any radiometric noise model assuming an ap-proximately Gaussian noise distribution.
4.2. Geometric Alignment
The HDR reconstruction is performed in a virtual refer-ence space, corresponding to a virtual sensor placed some-where in the focal plane. The virtual sensor dimensions are chosen to reflect the desired output frame, unconstrained by the resolution of the input frames. Using a geometric cali-bration procedure, an affine transform, Ts, is established for each sensor which maps sensor pixel coordinates, xs,i, to the coordinates of the reference output coordinate system, Xs,i= Ts(xs,i). In general, the transformed input pixel co-ordinates, Xs,i, are not integer-valued in the output image coordinates, and for general affine transformations the sam-ple locations Xs,iwill become irregularly positioned. An example for three sensor images is shown in Figure 1.
4.3. LPA Reconstruction
To reconstruct the HDR output frame, F, we estimate the radiant power for each pixel in the virtual sensor sep-arately in a non-parametric fashion using the transformed samples fˆs,i(Xs,i). For each output pixel j, with integer valued, regularly spaced coordinates Xj, we fit a polyno-mial model to observed samples fˆs,i(Xs,i) in a local neigh-bourhood. We perform the reconstruction of a single color channel, c = {R||G||B}, independently of the other color channels. Thus, unlike more advanced demosaicing meth-ods [11], our current implementation does not utilize cross-correlation between channels to improve the estimate. A local polynomial model of order M predicts the signal value at a point Xiin the neighbourhood of the fitted output location, Xjas
| z ( X i ; X j ; C ) = C T ϕ ¯ ( X j − X i ) |  | (1) |
|---|---|---|
|  | ϕ ¯ ( x ) = [ ϕ 0 ( x ) ; ϕ 1 ( x ) ; :::; ϕ M ( x )] T | (2) |
|  | C = ( C 0 ; C 1 ; :::; C M ) T | (3) |
$$ C = (C0; C1; :::; CM)T(3) $$
where ϕ¯(Xj) is a vector of 2D polynomials and C are the polynomial coefficients. For example, for M = 0,

---

## Page 4

defined using a smoothing window, Wh(X) =h12W(Xh),
L(Xj; C) = log(N(fk|z(Xk; Xj; C); ˆf2k)wk)
∑ = −1 (fk− CTϕ¯(Xj− Xk))2wk+ R 2
k ˆfk
where R represents terms independent of C and wkis the smoothing weight given by the window function, Wh(Xk), evaluated at sample Xk. The polynomial coefficients, C˜, maximizing the local likelihood function are found by the weighted least squares estimate
T
$$ = (Φ WΦ)−1ΦTWf¯ (5) $$
T
$$ where Φ = [ ϕ¯(Xj− X1) ; ϕ¯(Xj− X2)T:::ϕ¯(Xj− $$
XK)T]Tis a K × M + 1 design matrix, W = diag[w1;w2; :::;wK] is a K×K diagonal weight matrix,
σˆ σˆ
f1 f2 σˆfK
and   f ¯   = [ f 1 ; f 2 ; :::f K   ] T   .
Using the fitted polynomial coefficients,
mate the radiant power at the virtual sensor pixel as
ˆ
f   ( X ) =   z ( X ; X ;
j   j   j  
˜ estimated using the higher order coefficients C1and C˜2[5].
4.3.1 Parameters
The expected mean square error of the reconstructed im-age depends on a trade-off between bias and variance of the estimate. This trade-off is determined by: the order of the polynomial basis, M, the window function, W, and the
scale parameter, h. Finding the optimal balance between these parameters is, in the general case, very difficult and still an open problem in non-parametric statistics [22]. In-stead of relying on automatic selection of these parameters, we treat them as user parameters that are set depending on the sensor configuration and scene properties. Polynomial Order- Using a piecewise constant polyno-mial, M = 0, the estimator corresponds to an ordinary lo-cally weighted average of neighboring sensor observations ∑
$$ ˆ ∑k σˆwfikfk $$
k σˆfk
T smallest possible scale parameter, h, such that (Φ WΦ)−1
hG=h√R,B2, as there are more green samples per unit area.
4.4. CUDA Implementation
Our software implementation of the algorithm required several seconds for processing of reasonably sized frames. To attain real time processing speed for video data, we im-plemented the algorithm in CUDA. Two CUDA kernels are executed per frame. First, the raw 12-bit sensor images are converted to a half-float radiant power estimate, fˆs,i, and a variance estimate, ˆfˆs,i. Second, for each pixel in the output image, all samples overlapping the observation window are read from global GPU memory, their corresponding weights are computed, and the weighted least squares estimate of Equation (5) is calculated. To increase memory transfer ef-ficiency, page-locked and write-combined memory is used. Data is transferred to the GPU for kernel execution and then back to the CPU for disk storage. Two CUDA streams are used to allow for simultaneous data transfer and kernel exe-cution. If the sensors are aligned or related by purely trans-lational transforms, and if the output resolution matches the

---

## Page 5

input resolution, the spatial arrangement of samples in each observation window will be invariant across the image, and the window weights wican be precomputed once. This re-sults in a 2-3× speedup for a local constant fit M = 0.
### 5. Sensor noise model
In this section we describe a radiometric camera noise model assuming a linear digital response. We model the noise using a radiometric camera model inspired by previous methods considering radiometric camera calibra-tion [28, 9] and optimal HDR capture methods [10, 14]. Each observed digital pixel value, ys,i, is obtained us-ing an exposure time, ts, and an exposure scaling, nsthat is assumed constant over the image Is. The pixel response is a measurement of the radiant power reaching the image sensor, which we for convenience express as the number of photo-induced electrons collected per unit time, fs,i. The accumulated number of photoelectrons, es,i, collected at the pixel during the exposure time, ts, follows a Poisson distri-bution with expected value
E[es,i] = ts(as,insfs,i) (8)
and variance V ar[es,i] = E[es,i] where as,iis a per pixel factor due to non-uniform photo-response (photosensitive area). The recorded digital value, ys,i, is also affected by the sensor gain and signal independent readout noise,
ys,i= gs(es,i) + rs,i(g; t) (9)
where gsis the amplifier/sensor gain (proportional to ISO) and rs,i(g; t) is the readout noise, that can be dependent on both the gain, gs, and the exposure time, ts, settings of the sensor. For the Kodak KAI-04050 sensor used in our experimental setup, see Section 7 , we have found that the readout noise can be modeled by a pre-amplifier noise, ps,i, and a post amplifier quantization noise, qs[36]. We can thus form a parametric model of the readout noise as ds,i(g; t) = g(ps,i) + qs. However, for other image sen-sors other parametric models could be more appropriate, e.g. Hasinoff et al. [14] models the quantization noise with a more general post-amplifier noise. As we focus on video applications with frame rates of 25 fps or more, the dark current noise is neglected Before reconstruction of the HDR output frame, each digital pixel value, ys,i, is independently transformed to an estimate of the number of photoelectrons reaching the pixel per unit time, fˆs,i. To compensate for the readout noise bias (blacklevel), E[ds,i(g; t)], we subtract a bias frame, bs,i, from each observation. The bias frame is computed as the average of a large set of black images captured with the same camera settings as the observations but with the lens covered, so that no photons reach the sensor. The radiant
power fs,ican then be estimated as
ˆ ys,i− bs,i fs,i= (10) gstsas,ins
5.1. Variance estimate
We assume fˆs,ito follow a normal distribution with mean fs,iand standard deviation fˆ. For low light levels, photon shot noise is generally dominated by signal indepen-dent readout noise approximately following a normal distri-bution, and for brighter areas, with a high photon count, the Poisson distributed shot noise is well approximated by a normal distribution. Assuming no saturated or clipped pix-els values, the variance of fˆs,iis given by
V ar[ fˆs,i] = f2ˆ= gs2(V ar[es,igs2]) +t2sa2s,iV arn2s[rs,i(g; t)] (11)
variance estimate, ˆf2s,iˆ, as
2gstsas,insfˆ+ V ar[rs,i(g; t)] ˆfs.iˆ= gs2t2sa2s,in2s(12)
ances, f2s,iˆ, are assumed to be independent of each other.
5.2. Parameter calibration
s(13) E[y ] − E[b ] s,i s,igsE[es,i]
where the second equality follows from es,ibeing Poisson distributed shot noise with E[es,i] = V ar[es,i]. E[ys,i] and E[bs,i] can be estimated by averaged flat fields and the bias frame respectively, and V ar[bs,i] as described above.

---

## Page 6

(a) Gamma mapped reference image
(b) Left to right: LPA (M=1), Debayer first, Tocci et al.
(c) Top: LPA (M=1), and bottom: Tocci et al.
Figure 2: (a) Tone-mapped HDR reference image. (b): Comparisons of reconstructions using LPA, a Debayer-first method and the method of Tocci et al. LPA reconstruction shows less color fringes and less overall noise. (c): Contrary to Tocci et al., LPA has good inherent denoising abilities.
### 6. Algorithm evaluation
To evaluate our algorithm, we compare its performance against the recent real-time multi-sensor algorithm pre-sented by Tocciet al. [32], which performs debayering after HDR reconstruction, and an algorithm performing debayer-ing before HDR reconstruction and using linear radiomet-ric weights (as proposed by Mitsunaga and Nayar [26] for a linear camera response curve). The latter represents the more traditional exposure bracketing approach, where de-mosaicing is performed before HDR assembly. This will be referred to as Debayer-first. As stated in the introduction, Debayer-first methods have a serious drawback in that many pixel measurements may be affected by nearby saturated or
3.69 n1= 2 , n2= 27.78, simulating the setup presented by
lated four 10-bit sensors 3 f-stops apart, n0= 1, n1= 23, 6 n2= 2 , n3= 29, with a resolution of 512 × 764 pix-
els. The gain and readout noise were set to gs= 0:92 and ds,i∼ N(32; 18), corresponding to the measured sensor noise of a Canon PowerShot S5 [10]. The virtual sensors were perfectly aligned, with overlapping Bayer-CFA loca-tions. In Figure 2c, the reconstruction of our method (LPA) is compared to the method presented in [32]. The LPA re-construction exhibits low noise due to the adaptive filtering and data fusion between sensors, and reduces blur and color

---

## Page 7

 ( a )   L P A   ( t o p ) ,   v s .   T o c c i   e t   a l .  ( b )   G a m m a   m a p p e d   r e f e r e n c e   i m a g e  ( c )   L P A ,   M = 0 , 1 , 2  ( d )   L P A ,     h R , B = 0 . 3 ,   0 . 6 ,   0 . 9 
Figure 4: (a) Comparison between LPA reconstruction, M = 1, (top) and the method by Tocci et al. (bottom) from three simulated Canon 5D sensors 4 f-stops apart.b)Tone-mapped HDR reference image. c) LPA reconstructions of different polynomial order M = 0; 1; 2 with a fixed scale parameter, h = 0:2, h G R,B= 0:4. Using a constant polynomial (M = 0) yields blocky artifacts and color fringes close to sensor saturation and sharp edges. d) LPA reconstructions, M = 1, for different scale parameters, h R,B= 0:3; 0:6; 0:9. Using a low scale parameter leads to sharp reconstructions, but a larger scale parameter reduces image noise.
of Granados et al. our method handles arbitrary misalign-ments between the sensors. Figure 3: Reconstructed results from four perfectly aligned LPA parameters - Varying the polynomial order and exposures 4 f-stops apart captured with a Canon 40D cam- the scale parameter of the local polynomial model imposes era. (a) LPA Reconstruction and (b) the iterative method of a trade-off between image sharpness, noise reduction and Granados et al. [10]. processing speed. Figure 4b and 4c show reconstructions from three aligned virtual Kodak KAI-04050 sensors 4 f-artifacts by performing resampling and CFA interpolation stops apart and a resolution of 512×341 pixels. The close-jointly with reconstruction. This is expected, as the LPA ups show how the image fidelity varies for different choices algorithm uses a weighted least squares estimate based on of polynomial order the M and the scale parameter h de-all available sensor observations, while the method of Tocci scribed in Section 4.3.1. Using a constant fit, M = 0, leads et al. only considers observations from the highest exposed to unwanted block artifacts and color fringes, especially at non-saturated sensor. LPA reconstruction also gracefully edge features and in regions were one sensor is close to sat-handles transitional regions around sensor saturation points, uration. The difference between orderM = 1andM = 2is as a smoothing spatial filtering and a cross-sensor blending more subtle, with only small improvements in visual qual-are both inherent in the reconstruction. This has the benefit ity. A low value for the scale parameter, h, increases image of implicitly performing denoising. sharpness, while a large h reduces image noise. Similar results can also be seen in Figure 4a, where the methods are compared using three virtual exposures, 4 f-
### 7. Experimental validation
stops apart, each simulating a Canon EOS 5D sensor set to ISO400, corresponding to a gain ofg = 0:23and a readout sTo evaluate the the real world performance of our recon-noise with variance 6:5 as reported by [10]. struction algorithm, we employ a custom built multi-sensor To compare our method to the algorithm proposed by system. Four high quality CCD sensors, Kodak KAI-04050 Granados et al. [10] we captured four exposures four f- with a resolution of2336×1752pixels and RGB Bayer pat-stops apart with a Canon 40D camera, with calibrated gain tern CFA sampling, receive different amounts of light from 1:24(ISO 400), and readout noise variance64:2(14 bit sen- an optical system with a common lens, a four-way beam sor). For a fair comparison to our method we perform de- splitter and four different ND filters, see Figure 5, top. The

---

## Page 8

the ND-filters, cover a range of 1 : 212. This yields a dy-
namic range equivalent to 12 + 12 = 24 bits of linear res-olution, commonly referred to as “24 f-stops” of dynamic range. The dynamic range can be extended further by vary-ing the exposure times between the sensors. Each sensor is connected to a host computer through a CameraLink inter-face. The system allows for capture, processing and off-line storage of up to 32 frames per second at 4 Mpixels resolu-tion, amounting to around 1 GiB/s per second of raw data. Sensor misalignments were measured by imaging a checkerboard calibration target and computing the cross-sensor correlation. The misalignments were in our case translations (of a few pixels) and pure 2D rotations (in the order of fractions of a degree) and could be estimated with an accuracy within 0:1 pixels. The system was radiometri-cally calibrated as described in Section 5.2. The gain pa-rameter was estimated for each pixel separately, and then spatially averaged to find the sensor gain gs= 0:27DV =e. Readout noise was estimated for each pixel, with a cross-sensor spatial mean of ˆs,i= 72 e, and a standard deviation of ˆs,i≈ 11:8 e Figure 5 shows an example frame captured using our experimental HDR-video system. Using our CUDA im-plementation with M = 0, and an output resolution of 2336 × 1752 pixels, the four input frames are processed at 26 fps using pre-computed weights on an Nvidia GForce 680 GPU. As can be seen in the tone-mapped image, which is in full resolution to enable zooming in during on-screen viewing, the small rotational misalignments present in the hardware setup are not visible even though the weights are pre-computed. This is partly due to the smoothing inherent in the LPA method. Reconstructing the same frame with-out pre-computed weights achieves an interactive speed of 7 fps. The performance scales linearly with the number of pixels, so the corresponding figures for reconstructing to HD resolution frames of 1920 × 1080 are 51 fps and 14 fps respectively.
### 8. Conclusions and Future Work
This paper presented a novel method for HDR recon-struction from multi-sensor and multi-exposure images that performs all steps in the traditional HDR imaging pipeline in a single step. The method is based on a sensor noise model adapted to multi-sensor systems and a novel HDR assembly algorithm based on local polynomial approxima-tions. We also presented an overview of a novel HDR video camera and showed how our algorithm achieves real-time performance. As future work, we will further explore more
Sensor 1 Beam splitters Sensor 2
| Sensor 3 | Sensor 3 |
|---|---|
| Sensor 4 | Lens |
Figure 5: Top: An overview of our experimental HDR video camera. Bottom: a full resolution locally tone mapped frame from a video sequence captured with the HDR video camera. The image is reconstructed from four 4 Mpixel sen-sors at 26 fps. Bottom-right: Four gamma mapped images 2 f-stops apart from the same HDR-image.
advanced statistical denoising methods using anisotropic smoothing supports [5, 30].
### 9. Acknowledgements
We would like to thank Per Larsson for the construction and setup of camera hardware and invaluable help in the lab, Anders Ynnerman for insightful discussions and proof reading of the manuscript, and the anonymous reviewers for helping us improving the paper. This work was funded by the Swedish Foundation for Strategic Research through grant IIS11-0081, Link¨oping University Center for Indus-trial Information Technology (CENIIT), and the Swedish Research Council through the Linnaeus Center CADICS.
### References
[1] M. Aggarwal and N. Ahuja. Split aperture imaging for high dynamic range. International Journal of Computer Vision, 58(1):7–17, 2004. [2] B. Ajdin, M. B. Hullin, C. Fuchs, H.-P. Seidel, and H. P. A. Lensch. Demosaicing by smoothing along 1-d features. In Proc. of CVPR 2008, 2008. [3] A. O. Aky¨uz and E. Reinhard. Noise reduction in high dy-namic range imaging. Journal of Visual Communication and Image Representation, 18(5):366 – 376, 2007.

### Extracted Citations (JSON)
```json
[
  {
    "id": "1",
    "text": "M. Aggarwal and N. Ahuja. Split aperture imaging for high dynamic range. International Journal of Computer Vision, 58(1):7–17, 2004."
  },
  {
    "id": "2",
    "text": "B. Ajdin, M. B. Hullin, C. Fuchs, H.-P. Seidel, and H. P. A. Lensch. Demosaicing by smoothing along 1-d features. In Proc. of CVPR 2008, 2008."
  },
  {
    "id": "3",
    "text": "A. O. Aky¨uz and E. Reinhard. Noise reduction in high dy- namic range imaging. Journal of Visual Communication and Image Representation, 18(5):366 – 376, 2007."
  }
]
```

---

## Page 9

[4] R. v. S. Anton Kachatou. Dynamic range enhancement al-gorithms for cmos sensors with non-destructive readout. In IEEE International Worshop on Imaging Systems and Tech-niques, 2008. [5] J. Astola, V. Katkovnik, and K. Egiazarian. Local Approx-imation Techniques in Signal and Image Processing. SPIE Publications, 2006. [6] P. Debevec and J. Malik. Recovering high dynamic range radiance maps from photographs. In Proc. of ACM SIG-GRAPH, pages 369–378. ACM, ACM, 1997. [7] S. Farsiu, M. Elad, and P. Milanfar. Multiframe demosaic-ing and super-resolution of color images. Image Processing, IEEE Transactions on, 15(1):141–159, 2006. [8] S. Farsiu, M. D. Robinson, M. Elad, and P. Milanfar. Fast and robust multi-frame super-resolution. IEEE Transactions on Image Processing, 13(10):1327–1344, Oct. 2004. [9] A. Foi, M. Trimeche, V. Katkovnik, and K. Egiazarian. Practical poissonian-gaussian noise modeling and fitting for single-image raw-data. IEEE Transactions on Image Pro-cessing, 17(10):1737–1754, 2008. [10] M. Granados, B. Ajdin, M. Wand, C. Theobalt, H. Seidel, and H. Lensch. Optimal HDR reconstruction with linear dig-ital cameras. In in Proc. of CVPR. IEEE, IEEE, 2010. [11] B. Gunturk, J. Glotzbach, Y. Altunbasak, R. Schafer, and R. Mersereau. Demosaicking: color filter array interpola-tion. IEEE Signal Processing Magazine, 22(1):44–54, Jan. 2005. [12] B. K. Gunturk and M. Gevrekci. High-Resolution Image Reconstruction From Multiple Differently Exposed Images. IEEE Signal Processing Letters, 13(4):197–200, July 2006. [13] R. Hardie. A fast image super-resolution algorithm using an adaptive Wiener filter. IEEE Transactions on Image Process-ing, 16(12):2953–64, Dec. 2007. [14] S. Hasinoff, F. Durand, and W. Freeman. Noise-optimal cap-ture for high dynamic range photography. In in Proc. CVPR, pages 553 –560, 2010. [15] K. Hirakawa. Color filter array image analysis for joint denoising and demosaicking. In R. Lukac, editor, Single-Sensor Imaging: Methods and Applications for Digital Cam-eras. CRC Press, 2008. [16] T. P. K. Hirakawa. Joint demosaicing and denoising. IEEE Trans. Image Processing, 2006. [17] S. Kang, M. Uyttendaele, S. Winder, and R. Szeliski. High dynamic range video. ACM Transactions on Graphics (TOG) (Proceedings of SIGGRAPH 2003), 22(3):319–325, 2003. [18] W.-C. Kao. High dynamic range imaging by fusing multiple raw images and tone reproduction. IEEE Transactions on Consumer Electronics, 2008. [19] K. Kirk and H. Andersen. Noise characterization of weight-ing schemes for combination of multiple exposures. In Proc. British Machine Vision Conference (BMVC), volume 3, pages 1129–1138, 2006. [20] H. Knutsson and C.-F. Westin. Normalized and differential convolution. In Proc. of CVPR, pages 515–523, 1993. [21] P. Lancaster and K. Salkauskasr. Surfaces generated by mov-ing least squares methods. Mathematics of Computation, 87:141–158, 1981.
[22] C. Loader. Local regression and likelihood. New York: Springer-Verlag, 1999. [23] S. Mann and R. W. Picard. On Being ’undigital’ With Digital Cameras: Extending Dynamic Range By Combining Differ-ently Exposed Pictures. In Proc. of IS&T Ann. Conf., 1995. [24] P. Milanfar, editor. Super-resolution Imaging. CRC Press, 2010. [25] P. Milanfar. A tour of modern image filtering. to appear in IEEE Signal Processing Magazine, 2011. [26] T. Mitsunaga and S. K. Nayar. Radiometric Self Calibration. In Proc. of CVPR, volume 1, pages 374–380, 1999. [27] K. Myszkowski, R. Mantiuk, and G. Krawczyk. High Dy-namic Range Video. Morgan & Claypool, 2008. [28] Y. Reibel, M. Jung, M. Bouhifd, B. Cunin, and C. Draman. Ccd or cmos camera noise characterisation. The European Physical Journal - Applied Physics, 21:75–80, 0 2003. [29] E. Reinhard, W. Heidrich, S. Pattanaik, P. Debevec, G. Ward, and K. Myszkowski. High dynamic range imaging: acquisi-tion, display, and image-based lighting. Morgan Kaufmann, 2010. [30] H. Takeda, S. Farsiu, and P. Milanfar. Kernel regression for image processing and reconstruction. IEEE Transactions On Image Processing, 16(2):349–366, 2007. [31] R. Tibshirani and T. Hastie. Local likelihood estimation. Journal of the American Statistical Association, 82(398):pp. 559–567, 1987. [32] M. D. Tocci, C. Kiser, N. Tocci, and P. Sen. A Versatile HDR Video Production System. ACM Transactions on Graphics (TOG) (Proceedings of SIGGRAPH 2011), 30(4), 2011. [33] C. Tomasi and R. Manduchi. Bilateral filtering for gray and color images. In Sixth International Conference on Computer Vision (ICCV), pages 839–846. Narosa Publishing House, 1998. [34] J. Unger and S. Gustavson. High-dynamic-range video for photometric measurement of illumination. Proc. of SPIE, 6501:65010E, 2007. [35] J. Unger, S. Gustavson, M. Ollila, and M. Johannesson. A real time light probe. In In Proceedings of the 25th Euro-graphics Annual Conference, volume Short Papers and In-teractive Demos, pages 17–21, 2004. [36] B. Widrow, L. Fellow, I. Kolll, S. Member, and M. chang Liu. Statistical theory of quantization. IEEE Trans. on In-strumentation and Measurement, pages 353–361, 1996. [37] H. Zimmer, A. Bruhn, and J. Weickert. Freehand HDR imaging of moving scenes with simultaneous resolution en-hancement. Computer Graphics Forum (Proceedings of Eu-rographics), 30(2):405–414, 2011.

### Extracted Citations (JSON)
```json
[
  {
    "id": "4",
    "text": "R. v. S. Anton Kachatou. Dynamic range enhancement al- gorithms for cmos sensors with non-destructive readout. In IEEE International Worshop on Imaging Systems and Tech- niques, 2008."
  },
  {
    "id": "5",
    "text": "J. Astola, V. Katkovnik, and K. Egiazarian. Local Approx- imation Techniques in Signal and Image Processing. SPIE Publications, 2006."
  },
  {
    "id": "6",
    "text": "P. Debevec and J. Malik. Recovering high dynamic range radiance maps from photographs. In Proc. of ACM SIG- GRAPH, pages 369–378. ACM, ACM, 1997."
  },
  {
    "id": "7",
    "text": "S. Farsiu, M. Elad, and P. Milanfar. Multiframe demosaic- ing and super-resolution of color images. Image Processing, IEEE Transactions on, 15(1):141–159, 2006."
  },
  {
    "id": "8",
    "text": "S. Farsiu, M. D. Robinson, M. Elad, and P. Milanfar. Fast and robust multi-frame super-resolution. IEEE Transactions on Image Processing, 13(10):1327–1344, Oct. 2004."
  },
  {
    "id": "9",
    "text": "A. Foi, M. Trimeche, V. Katkovnik, and K. Egiazarian. Practical poissonian-gaussian noise modeling and fitting for single-image raw-data. IEEE Transactions on Image Pro- cessing, 17(10):1737–1754, 2008."
  },
  {
    "id": "10",
    "text": "M. Granados, B. Ajdin, M. Wand, C. Theobalt, H. Seidel, and H. Lensch. Optimal HDR reconstruction with linear dig- ital cameras. In in Proc. of CVPR. IEEE, IEEE, 2010."
  },
  {
    "id": "11",
    "text": "B. Gunturk, J. Glotzbach, Y. Altunbasak, R. Schafer, and R. Mersereau. Demosaicking: color filter array interpola- tion. IEEE Signal Processing Magazine, 22(1):44–54, Jan. 2005."
  },
  {
    "id": "12",
    "text": "B. K. Gunturk and M. Gevrekci. High-Resolution Image Reconstruction From Multiple Differently Exposed Images. IEEE Signal Processing Letters, 13(4):197–200, July 2006."
  },
  {
    "id": "13",
    "text": "R. Hardie. A fast image super-resolution algorithm using an adaptive Wiener filter. IEEE Transactions on Image Process- ing, 16(12):2953–64, Dec. 2007."
  },
  {
    "id": "14",
    "text": "S. Hasinoff, F. Durand, and W. Freeman. Noise-optimal cap- ture for high dynamic range photography. In in Proc. CVPR, pages 553 –560, 2010."
  },
  {
    "id": "15",
    "text": "K. Hirakawa. Color filter array image analysis for joint denoising and demosaicking. In R. Lukac, editor, Single- Sensor Imaging: Methods and Applications for Digital Cam- eras. CRC Press, 2008."
  },
  {
    "id": "16",
    "text": "T. P. K. Hirakawa. Joint demosaicing and denoising. IEEE Trans. Image Processing, 2006."
  },
  {
    "id": "17",
    "text": "S. Kang, M. Uyttendaele, S. Winder, and R. Szeliski. High dynamic range video. ACM Transactions on Graphics (TOG) (Proceedings of SIGGRAPH 2003), 22(3):319–325, 2003."
  },
  {
    "id": "18",
    "text": "W.-C. Kao. High dynamic range imaging by fusing multiple raw images and tone reproduction. IEEE Transactions on Consumer Electronics, 2008."
  },
  {
    "id": "19",
    "text": "K. Kirk and H. Andersen. Noise characterization of weight- ing schemes for combination of multiple exposures. In Proc. British Machine Vision Conference (BMVC), volume 3, pages 1129–1138, 2006."
  },
  {
    "id": "20",
    "text": "H. Knutsson and C.-F. Westin. Normalized and differential convolution. In Proc. of CVPR, pages 515–523, 1993."
  },
  {
    "id": "21",
    "text": "P. Lancaster and K. Salkauskasr. Surfaces generated by mov- ing least squares methods. Mathematics of Computation, 87:141–158, 1981."
  },
  {
    "id": "22",
    "text": "C. Loader. Local regression and likelihood. New York: Springer-Verlag, 1999."
  },
  {
    "id": "23",
    "text": "S. Mann and R. W. Picard. On Being ’undigital’ With Digital Cameras: Extending Dynamic Range By Combining Differ- ently Exposed Pictures. In Proc. of IS&T Ann. Conf., 1995."
  },
  {
    "id": "24",
    "text": "P. Milanfar, editor. Super-resolution Imaging. CRC Press, 2010."
  },
  {
    "id": "25",
    "text": "P. Milanfar. A tour of modern image filtering. to appear in IEEE Signal Processing Magazine, 2011."
  },
  {
    "id": "26",
    "text": "T. Mitsunaga and S. K. Nayar. Radiometric Self Calibration. In Proc. of CVPR, volume 1, pages 374–380, 1999."
  },
  {
    "id": "27",
    "text": "K. Myszkowski, R. Mantiuk, and G. Krawczyk. High Dy- namic Range Video. Morgan & Claypool, 2008."
  },
  {
    "id": "28",
    "text": "Y. Reibel, M. Jung, M. Bouhifd, B. Cunin, and C. Draman. Ccd or cmos camera noise characterisation. The European Physical Journal - Applied Physics, 21:75–80, 0 2003."
  },
  {
    "id": "29",
    "text": "E. Reinhard, W. Heidrich, S. Pattanaik, P. Debevec, G. Ward, and K. Myszkowski. High dynamic range imaging: acquisi- tion, display, and image-based lighting. Morgan Kaufmann, 2010."
  },
  {
    "id": "30",
    "text": "H. Takeda, S. Farsiu, and P. Milanfar. Kernel regression for image processing and reconstruction. IEEE Transactions On Image Processing, 16(2):349–366, 2007."
  },
  {
    "id": "31",
    "text": "R. Tibshirani and T. Hastie. Local likelihood estimation. Journal of the American Statistical Association, 82(398):pp. 559–567, 1987."
  },
  {
    "id": "32",
    "text": "M. D. Tocci, C. Kiser, N. Tocci, and P. Sen. A Versatile HDR Video Production System. ACM Transactions on Graphics (TOG) (Proceedings of SIGGRAPH 2011), 30(4), 2011."
  },
  {
    "id": "33",
    "text": "C. Tomasi and R. Manduchi. Bilateral filtering for gray and color images. In Sixth International Conference on Computer Vision (ICCV), pages 839–846. Narosa Publishing House, 1998."
  },
  {
    "id": "34",
    "text": "J. Unger and S. Gustavson. High-dynamic-range video for photometric measurement of illumination. Proc. of SPIE, 6501:65010E, 2007."
  },
  {
    "id": "35",
    "text": "J. Unger, S. Gustavson, M. Ollila, and M. Johannesson. A real time light probe. In In Proceedings of the 25th Euro- graphics Annual Conference, volume Short Papers and In- teractive Demos, pages 17–21, 2004."
  },
  {
    "id": "36",
    "text": "B. Widrow, L. Fellow, I. Kolll, S. Member, and M. chang Liu. Statistical theory of quantization. IEEE Trans. on In- strumentation and Measurement, pages 353–361, 1996."
  },
  {
    "id": "37",
    "text": "H. Zimmer, A. Bruhn, and J. Weickert. Freehand HDR imaging of moving scenes with simultaneous resolution en- hancement. Computer Graphics Forum (Proceedings of Eu- rographics), 30(2):405–414, 2011."
  }
]
```